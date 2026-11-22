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
//! results of a harness whose call ids name one execution. Pi's native ids
//! do.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use kernel::applicability::EvalBudget;
use memory_store::{BlockKind, OutputKind, ResultBlockKind, WireBlock};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::native::{completes_failure_line, is_output_failure_line, printable};
use super::{CAPTURE_SOURCE_KIND, COMMIT_WAIT, capture_enabled};
use crate::healing::SerializerProfile;
use crate::kernel_routes::{self, ProjectBinding};
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
const MAX_WORD_BYTES: usize = 64;
/// The tool's name as a record shows it, in bytes.
const MAX_TOOL_BYTES: usize = 32;
/// A tool call's id and a session's id as a record's text shows them, in
/// bytes. The record's id and its lineage derive from the whole ids.
const MAX_ID_BYTES: usize = 96;
/// The path and the error name a record keeps, in bytes each.
const MAX_TOKEN_BYTES: usize = 96;
/// The record's text, in bytes. The search tool renders 1,024 bytes of a
/// fact's text, a line break, and its rationale together, so the text and
/// the first fields of the rationale fit.
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
/// The `echo` commands of one command line read for a printed exit status.
const MAX_STATUS_ECHOES: usize = 8;
/// Records one session writes in one project, resolved copies included.
const MAX_RECORDS_PER_SESSION: usize = 64;
/// Command results one pass hands off, the earliest unjudged ones first, so
/// the results of one command are judged in the order they ran.
const MAX_RESULTS_PER_PASS: usize = 256;
/// The bytes of commands and outputs one pass copies for its checkpoint.
const MAX_HANDOFF_BYTES: usize = 8 * 1024 * 1024;
/// Tool calls one session's memo remembers, and the command results at the
/// end of a window that a pass considers. The two are equal, so a call the
/// memo evicts has left the results a later pass considers.
const MEMO_CALLS_PER_SESSION: usize = 1024;
const MEMO_SESSIONS: usize = 256;
/// Checkpoints that read results at one time. Their judgments run one at a
/// time under the capture publication gate, so more of them add no work
/// done.
const MAX_CHECKPOINTS: usize = 8;
// A memo at its bound of sessions always holds an entry that no checkpoint
// pins, which a new session's entry replaces.
const _: () = assert!(MAX_CHECKPOINTS < MEMO_SESSIONS);
/// The bytes one checkpoint holds beside its copied results while it reads
/// one of them: the redacted command, and the redactor's copies of the
/// output.
const CHECKPOINT_SCRATCH_BYTES: usize = 8 * MAX_OUTPUT_BYTES;
/// Marks a line or a word cut to its byte limit.
const CUT_MARK: &str = "…";
/// The sentence a resolved record's first line ends with.
const RESOLVED_NOTE: &str = " Resolved: a later run of this command reported no failure.";
/// What follows the program's name in a record whose command line holds
/// more than the words shown.
const ARGUMENTS_NOTE: &str = " [arguments left out]";
/// What a record shows for a command line that begins with no plain word.
const LEFT_OUT: &str = "[left out]";
/// Opens each line of a tool's output that a record quotes, so a reader of
/// a search result tells quoted output from the record's own lines.
const QUOTE_MARK: &str = "> ";

/// The resident bytes this module retains at its bounds: each session's
/// call memo, at the 2,048 slots its set and its queue grow to, and for
/// each checkpoint that runs the results it copied, their entries, and its
/// scratch.
pub(crate) const RETAINED_BYTES_BOUND: u64 = (MEMO_SESSIONS * MEMO_CALLS_PER_SESSION * 80
    + MAX_CHECKPOINTS
        * (MAX_HANDOFF_BYTES
            + MAX_RESULTS_PER_PASS * size_of::<Pending>()
            + CHECKPOINT_SCRATCH_BYTES)) as u64;

/// What makes a result a failure, the strongest ground first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Evidence {
    /// The harness marked the result as an error, or the result states a
    /// nonzero exit code.
    ToolStatus,
    /// The command line printed its own exit status, and it is nonzero.
    PrintedStatus,
    /// The shell reported a program of the command line that it did not
    /// find.
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

/// One line chosen from a failed result, within [`MAX_LINE_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Kept {
    text: String,
    /// The line is one of the output's closing lines.
    closing: bool,
}

/// A failure as the output states it, before the redactor has judged the
/// output.
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    evidence: Evidence,
    /// The nonzero exit code the result states or the command line printed,
    /// with the bytes of the line that holds it.
    exit_code: Option<(i64, Range<usize>)>,
    /// The lines that state an error, in output order, and then the closing
    /// lines that are none of them, each with the bytes it stands at. A
    /// text that several chosen lines share stands once.
    lines: Vec<(Kept, Range<usize>)>,
}

/// What a record keeps of a failed result's output.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Excerpt {
    /// The redactor found nothing in the output, so its chosen lines stand
    /// in the record.
    Kept(Vec<Kept>),
    /// The redactor found credential-shaped content in the output. The
    /// record holds no line of it.
    Finding,
    /// The redactor's scan of the output did not finish. The record holds
    /// no text of it.
    Unscanned,
}

/// A failed result as a record states it. Every text here has passed the
/// redactor.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Failure {
    evidence: Evidence,
    exit_code: Option<i64>,
    /// The first path and the first error name among the chosen lines that
    /// hold no finding.
    path: Option<String>,
    error: Option<String>,
    excerpt: Excerpt,
}

/// What one tool result says about its command.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// The result reports no failure.
    Passed,
    Failed(Failure),
}

/// One command result of the window that the session's memo has not seen.
struct Pending {
    /// The name of the tool call in the memo.
    digest: u64,
    /// The copied inputs of a result that is read. `None` for a result that
    /// changes no record whatever it holds, whose call is settled unread.
    input: Option<Input>,
}

/// What the task reads of one command result, copied from the window within
/// [`MAX_OUTPUT_BYTES`] for the command and for the text, so the task holds
/// no message of the pass.
struct Input {
    /// The id that names the execution: Pi's native tool call id.
    call: String,
    tool: String,
    command: String,
    text: String,
    /// The harness marked the result as an error.
    flagged: bool,
    /// When the harness wrote the result, in milliseconds.
    at_ms: i64,
}

/// A command line as a record names it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Command {
    /// The program's name and its first plain operand, and
    /// [`ARGUMENTS_NOTE`] when the command line holds more; [`LEFT_OUT`]
    /// when the command line begins with no plain word.
    shown: String,
    /// The digest of the project and the redacted, normalized command line,
    /// which names the lineage of the command's records.
    digest: String,
}

/// One failed or passing command result as the detached task read it.
struct Observed {
    call: String,
    /// The tool's name as a record shows it.
    tool: String,
    command: Command,
    at_ms: i64,
    outcome: Outcome,
}

/// The tool calls of one session that its checkpoints have judged.
#[derive(Default)]
struct SessionCalls {
    /// Distinguishes this entry from one created after the session's delete.
    epoch: u64,
    calls: HashSet<u64>,
    /// The judged calls from the earliest learned to the latest.
    order: VecDeque<u64>,
}

/// Tool calls already judged, by project and session. The memo spares a
/// pass the reading of results an earlier pass judged; the Kernel's commit
/// log holds the receipt of each record's commit, so a call judged again
/// after a restart writes nothing.
#[derive(Default)]
pub(crate) struct FailureMemo {
    sessions: HashMap<String, SessionCalls>,
    /// Sessions in the order their entries were created.
    order: VecDeque<String>,
    next_epoch: u64,
    /// The sessions whose checkpoint is admitted and has not finished, each
    /// with the epoch of the entry that checkpoint pinned. A session's
    /// delete removes its entry and leaves its place here, so the session
    /// admits its next checkpoint once the running one has ended.
    active: HashMap<String, u64>,
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

    /// Whether a new checkpoint of `session` waits: one of the session
    /// runs, or [`MAX_CHECKPOINTS`] run.
    fn busy(&self, session: &str) -> bool {
        self.active.len() >= MAX_CHECKPOINTS || self.active.contains_key(session)
    }

    /// Admits one checkpoint of the session and returns the epoch of the
    /// session's entry, creating the entry when absent. A new entry at the
    /// session limit takes the place of the oldest entry whose session runs
    /// no checkpoint; the replaced session's calls are judged again on its
    /// next request, where the Kernel's receipts stop a second write.
    fn pin(&mut self, session: &str) -> u64 {
        if !self.sessions.contains_key(session) {
            if self.sessions.len() >= MEMO_SESSIONS {
                let idle = self
                    .order
                    .iter()
                    .position(|kept| !self.active.contains_key(kept));
                if let Some(victim) = idle.and_then(|index| self.order.remove(index)) {
                    self.sessions.remove(&victim);
                }
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
        let epoch = self.sessions.get(session).map_or(0, |entry| entry.epoch);
        self.active.insert(session.to_owned(), epoch);
        epoch
    }

    /// Ends the checkpoint that pinned `epoch` of the session.
    fn release(&mut self, session: &str, epoch: u64) {
        if self.active.get(session) == Some(&epoch) {
            self.active.remove(session);
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

    /// The number of checkpoints that run.
    #[cfg(test)]
    pub(crate) fn active_for_test(&self) -> usize {
        self.active.len()
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
/// single spaces, when it is at most [`MAX_OUTPUT_BYTES`]. The length is
/// read before a byte is copied.
fn command_of(input: &Value) -> Option<String> {
    match command_field(input)? {
        Value::String(text) => (text.len() <= MAX_OUTPUT_BYTES).then(|| text.clone()),
        Value::Array(words) => {
            let words = || words.iter().filter_map(Value::as_str);
            let bytes: usize = words().map(|word| word.len() + 1).sum();
            (bytes <= MAX_OUTPUT_BYTES + 1).then(|| words().collect::<Vec<_>>().join(" "))
        }
        _ => None,
    }
}

/// The text of a tool result and whether the harness marked it as an error:
/// the text of a `text` or `error_text` result, and the text blocks of a
/// `content` or `error_content` result joined by line breaks. `None` for a
/// result that is no text (a `json` or `error_json` value, which Pi's codec
/// never gives, and a result the harness denied) and for text longer than
/// [`MAX_OUTPUT_BYTES`], which source capture retains no span of. The
/// length is read before a byte is copied.
fn text_of(output: &OutputKind) -> Option<(String, bool)> {
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
    let whole = |text: &String| (text.len() <= MAX_OUTPUT_BYTES).then(|| text.clone());
    match output {
        OutputKind::Text { text } => Some((whole(text)?, false)),
        OutputKind::ErrorText { text } => Some((whole(text)?, true)),
        OutputKind::Content { blocks } => Some((joined(blocks)?, false)),
        OutputKind::ErrorContent { blocks } => Some((joined(blocks)?, true)),
        OutputKind::Json { .. }
        | OutputKind::ErrorJson { .. }
        | OutputKind::ExecutionDenied { .. } => None,
    }
}

/// Each character of `text` with the quoting it stands in, by the shell's
/// reading of single quotes, double quotes, and backslash escapes: `None`
/// for a character outside them, the quote character for one inside a
/// quoted span and for the quotes that open and close it, and a backslash
/// for a backslash and the character it escapes. The reading follows quotes
/// and backslashes and no other shell grammar, so the text of a
/// here-document is read as command words.
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
fn unquoted<'a>(text: &'a str, marks: &'a [char]) -> impl Iterator<Item = usize> + 'a {
    quoting(text)
        .filter(|(_, ch, state)| state.is_none() && marks.contains(ch))
        .map(|(index, ..)| index)
}

/// The characters that end a shell word outside quotes.
const WORD_ENDS: [char; 7] = [' ', '\t', '\n', '\r', ';', '&', '|'];

/// The end of the shell word `text` starts with: the first whitespace, `;`,
/// `&`, or `|` outside quotes and backslash escapes, or the end of the text.
/// The scan stops at that character.
fn word_end(text: &str) -> usize {
    unquoted(text, &WORD_ENDS).next().unwrap_or(text.len())
}

/// Whether a shell word holds a command substitution, whose text the word
/// reader has no reading of.
fn substitutes(word: &str) -> bool {
    word.contains("$(") || word.contains('`')
}

/// What follows a leading `cd <path> &&` or `cd <path>;` step, where the
/// path is one shell word without a command substitution.
fn after_cd(text: &str) -> Option<&str> {
    let path = text.strip_prefix("cd")?;
    if !path.starts_with(char::is_whitespace) {
        return None;
    }
    let path = path.trim_start();
    let end = Some(word_end(path)).filter(|end| *end > 0 && !substitutes(&path[..*end]))?;
    let tail = path[end..].trim_start();
    let after = tail.strip_prefix("&&").or_else(|| tail.strip_prefix(';'))?;
    Some(after.trim_start())
}

/// What follows a leading `NAME=value` assignment, where the value is one
/// shell word without a command substitution that whitespace ends.
fn after_assignment(text: &str) -> Option<&str> {
    let name = text.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))?;
    if name == 0 || text.starts_with(|ch: char| ch.is_ascii_digit()) {
        return None;
    }
    let value = text[name..].strip_prefix('=')?;
    let end = word_end(value);
    if substitutes(&value[..end]) {
        return None;
    }
    let after = &value[end..];
    after
        .starts_with(char::is_whitespace)
        .then(|| after.trim_start())
}

/// `text` with each whitespace run that stands outside quotes and backslash
/// escapes as one space, and without such whitespace at its ends. Whitespace
/// inside a quoted or escaped span is an argument's own text and stays.
fn folded(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut gap = false;
    for (_, ch, state) in quoting(text) {
        if state.is_none() && ch.is_whitespace() {
            gap = !out.is_empty();
            continue;
        }
        if std::mem::take(&mut gap) {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// The command without what varies between runs of one operation: leading
/// `cd <path> &&` and `cd <path>;` steps, leading `NAME=value` assignments,
/// and the width of whitespace runs between words. A step or an assignment
/// that ends the command stays, and so does one whose word holds a command
/// substitution.
fn normalized(command: &str) -> String {
    let mut rest = command.trim_start();
    while let Some(after) = after_cd(rest)
        .or_else(|| after_assignment(rest))
        .filter(|after| !after.is_empty())
    {
        rest = after;
    }
    folded(rest)
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

/// The tool's name as a record shows it: whitespace runs as single spaces,
/// within [`MAX_TOOL_BYTES`]. The words are joined until they pass the
/// limit, so the work is bounded by it.
fn tool_shown(name: &str) -> String {
    let mut shown = String::new();
    for word in name.split_whitespace() {
        if !shown.is_empty() {
            shown.push(' ');
        }
        shown.push_str(word);
        if shown.len() > MAX_TOOL_BYTES {
            break;
        }
    }
    clipped(&shown, MAX_TOOL_BYTES)
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
            shown: LEFT_OUT.to_owned(),
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

/// The characters that part two commands of a command line.
const COMMAND_ENDS: [char; 4] = [';', '|', '&', '\n'];

/// The commands of a command line, each with the character that ends it:
/// the text between two of [`COMMAND_ENDS`] outside quotes and backslash
/// escapes. `||` and `&&` leave an empty command between their two
/// characters.
fn steps(command: &str) -> Vec<(&str, Option<char>)> {
    let mut start = 0;
    let mut steps = Vec::new();
    for end in unquoted(command, &COMMAND_ENDS) {
        steps.push((&command[start..end], command[end..].chars().next()));
        start = end + 1;
    }
    steps.push((&command[start..], None));
    steps
}

/// The placeholder of the status parameter in an echo's template. A command
/// line holds no such character in a word.
const STATUS_PLACE: char = '\u{0}';

/// The length of the shell parameter that expands to an exit status at the
/// start of `text`: `$?`, `${?}`, and `${PIPESTATUS[<index>]}`, the status
/// of the last command and of the commands of the last pipeline.
fn status_parameter(text: &str) -> Option<usize> {
    if text.starts_with("$?") {
        return Some(2);
    }
    if text.starts_with("${?}") {
        return Some(4);
    }
    let index = text.strip_prefix("${PIPESTATUS[")?;
    let close = index.find("]}")?;
    let plain = index[..close]
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'@' || byte == b'*');
    plain.then_some("${PIPESTATUS[".len() + close + 2)
}

/// The text an `echo` command prints around the exit status it prints, when
/// `step` is an `echo` whose words hold a status parameter the shell
/// expands: outside single quotes and backslash escapes. The text before
/// and after the parameter is returned as the shell prints it, without
/// quote characters and escaping backslashes and with whitespace runs as
/// single spaces, when exactly one parameter expands; `Some(None)` is an
/// echo of several statuses, whose line is left unread. The words end at a
/// redirection, with the descriptor number before it, and at the `)` that
/// closes a subshell. A parameter in single quotes or behind a backslash is
/// text the echo prints as written.
fn status_echo(step: &str) -> Option<Option<(String, String)>> {
    let step = step.trim_start_matches(|ch: char| ch.is_whitespace() || "({".contains(ch));
    let words = step
        .strip_prefix("echo")
        .filter(|words| words.starts_with(char::is_whitespace))?;
    let end = unquoted(words, &['>', ')']).next().unwrap_or(words.len());
    let cut = &words[..end];
    // `1>&2` names a descriptor, which is no word of the echo.
    let descriptor = cut.trim_end_matches(|ch: char| ch.is_ascii_digit());
    let words = match words[end..].starts_with('>')
        && descriptor.len() < cut.len()
        && descriptor.ends_with(char::is_whitespace)
    {
        true => descriptor,
        false => cut,
    };
    let mut template = String::new();
    let mut statuses = 0;
    let mut skip = 0;
    let mut escaping = false;
    for (index, ch, state) in quoting(words) {
        if index < skip {
            continue;
        }
        let expands = ch == '$' && matches!(state, None | Some('"'));
        if let Some(length) = expands.then(|| status_parameter(&words[index..])).flatten() {
            statuses += 1;
            skip = index + length;
            template.push(STATUS_PLACE);
            continue;
        }
        // The quotes that open and close a span and the backslash that
        // escapes a character are the shell's own and print nothing.
        let quote = matches!(state, Some(quote) if quote == ch && quote != '\\');
        let escaper = state == Some('\\') && ch == '\\' && !escaping;
        escaping = escaper;
        if !quote && !escaper && ch != STATUS_PLACE {
            template.push(ch);
        }
    }
    if statuses == 0 {
        return None;
    }
    if statuses > 1 {
        return Some(None);
    }
    let template = collapsed(&template);
    let (before, after) = template.split_once(STATUS_PLACE)?;
    Some(Some((before.to_owned(), after.to_owned())))
}

/// The nonzero exit status the command line printed, with the line that
/// holds it. The first [`MAX_STATUS_ECHOES`] `echo` commands that print one
/// status are read (see [`status_echo`]). An echo's line is the last line
/// of the output that is the echo's text with one or more numbers in the
/// place of the parameter, whitespace runs as single spaces; an echo of
/// the parameter alone is read from the last non-empty line alone, since a
/// bare number stands in other lines of an output too. The greatest number
/// over the echoes is the status. The shell printed each for the command
/// before its echo, so a nonzero one has the strength of the tool's status.
fn printed_status(command: &str, lines: &[(&str, Range<usize>)]) -> Option<(i64, Range<usize>)> {
    let echoes: Vec<(String, String)> = steps(command)
        .into_iter()
        .filter_map(|(step, _)| status_echo(step).flatten())
        .take(MAX_STATUS_ECHOES)
        .collect();
    if echoes.is_empty() {
        return None;
    }
    let filled: Vec<(String, &Range<usize>)> = lines
        .iter()
        .filter(|(line, _)| !line.is_empty())
        .map(|(line, span)| (collapsed(line), span))
        .collect();
    let numbers = |middle: &str| -> Option<i64> {
        middle
            .split(' ')
            .map(|number| {
                (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
                    .then(|| number.parse::<i64>().ok())
                    .flatten()
            })
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .max()
    };
    echoes
        .iter()
        .filter_map(|(before, after)| {
            let bare = before.is_empty() && after.is_empty();
            let considered = if bare {
                filled.len().min(1)
            } else {
                filled.len()
            };
            filled
                .iter()
                .rev()
                .take(considered)
                .find_map(|(line, span)| {
                    let middle = line
                        .strip_prefix(before.as_str())?
                        .strip_suffix(after.as_str())?;
                    Some((numbers(middle)?, (*span).clone()))
                })
        })
        .max_by_key(|(status, _)| *status)
        .filter(|(status, _)| *status != 0)
}

/// Whether the command line hides an exit status from the tool: a pipe,
/// `||`, or `;` outside quotes and backslash escapes stands between two of
/// its commands. The shell reports a pipeline or a list by its last
/// command, so the tool's status covers that command alone. An `echo` that
/// prints an exit status is the line's own report and counts as no command
/// here, an `&&` chain ends at its first failure, and a `;` or a `|` at the
/// end of the line joins nothing.
fn hides_status(command: &str) -> bool {
    let steps = steps(command);
    let commands = |steps: &[(&str, Option<char>)]| {
        steps
            .iter()
            .any(|(step, _)| !step.trim().is_empty() && status_echo(step).is_none())
    };
    steps.iter().enumerate().any(|(index, (_, end))| {
        matches!(end, Some('|' | ';'))
            && commands(&steps[..=index])
            && commands(&steps[index + 1..])
    })
}

/// The words a shell prints when it finds no program of the name it was
/// given.
const MISSING_COMMAND: &str = "command not found";

/// The program a line reports as missing when the line is a shell's own
/// report: `<shell>: <program>: command not found`, with `line <n>:` before
/// the program for a script, as bash and sh print it, or
/// `<shell>: command not found: <program>`, as zsh prints it. The program
/// is one word.
fn missing_program(line: &str) -> Option<&str> {
    let program = match line.strip_suffix(": command not found") {
        Some(report) => report.rsplit_once(": ")?.1,
        None => line.split_once(": command not found: ")?.1,
    };
    (!program.is_empty() && !program.contains(char::is_whitespace)).then_some(program)
}

/// Whether `program` is a word of the command line, so the shell's report
/// of it concerns this command line.
fn names_program(command: &str, program: &str) -> bool {
    command
        .split(|ch: char| ch.is_whitespace() || ";|&(){}<>\"'`$=".contains(ch))
        .any(|word| word == program)
}

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
/// `Exception`, or the shell's [`MISSING_COMMAND`] words. The rule chooses
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

/// Whether the harness ended or refused the command itself, which leaves
/// the command's own outcome unknown: the result carries the harness's
/// error mark and its last non-empty line begins with one of
/// [`HARNESS_ENDED`], its whole text is [`HARNESS_BLOCKED`] or
/// `Tool <name> not found`, or it begins with [`HARNESS_INVALID`].
fn harness_decided(text: &str, flagged: bool) -> bool {
    if !flagged {
        return false;
    }
    let text = text.trim();
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

/// What a command's result reports: `None` for a result that reports no
/// failure.
///
/// The tool's own status decides first. The harness's error mark is a
/// failure, and so is a nonzero exit code that a status line states (see
/// [`stated_exit_code`]). For a result the harness marks, the last
/// non-empty line alone is read for the code, which is where the harness
/// states it. For any other result the lines read are the first non-empty
/// line, when it ends within the output's first [`MAX_FIRST_LINE_BYTES`],
/// and the last [`EXIT_CODE_CLOSING_LINES`] non-empty lines; the last of
/// them that states a code decides.
///
/// A result without that status, or with status 0, is the result of a
/// command line whose last command passed. Three grounds make it a failure,
/// each for a stated reason. A nonzero exit status that the command line
/// itself printed (see [`printed_status`]) is the shell's report of the
/// command before the `echo`, so it has the tool status's strength. The
/// shell's report of a program of the command line that it did not find
/// (see [`missing_program`]) names a command that did not run, whose status
/// a later command of the line replaced. A failure line, as capture reads
/// one in an output block, is a failure where the command line hides an
/// exit status (see [`hides_status`]): the tool's status then covers the
/// line's last command alone, and the output is the only report of the
/// others. A failure line in the output of a single command whose status is
/// 0, such as a search through a log, is that command's ordinary output.
///
/// The chosen lines are the first lines that state an error, each failure
/// line with the line that completes it, and then the closing lines of the
/// output. A line with a control character or a Unicode line separator is
/// left out, and a text that several lines share is chosen once.
fn outcome_of(command: &str, text: &str, flagged: bool) -> Option<Reported> {
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
    let shown = |line: &str| {
        !line.is_empty() && printable(line) && !line.contains(['\u{2028}', '\u{2029}'])
    };
    let states =
        |(line, span): &(&str, Range<usize>)| Some((stated_exit_code(line)?, span.clone()));
    let stated = {
        let mut closing = lines.iter().rev().filter(|(line, _)| !line.is_empty());
        match flagged {
            true => closing.next().and_then(states),
            false => {
                let first = lines
                    .iter()
                    .find(|(line, _)| !line.is_empty())
                    .filter(|(_, span)| span.end <= MAX_FIRST_LINE_BYTES);
                closing
                    .take(EXIT_CODE_CLOSING_LINES)
                    .find_map(states)
                    .or_else(|| first.and_then(states))
            }
        }
    };
    let code = stated.filter(|(code, _)| *code != 0);
    let mut fails = false;
    let mut missing = false;
    let mut errors: Vec<usize> = Vec::new();
    let mut chosen: HashSet<String> = HashSet::new();
    for (index, (line, _)) in lines.iter().enumerate().filter(|(_, line)| shown(line.0)) {
        let failure = is_output_failure_line(line);
        fails |= failure;
        missing |= missing_program(line).is_some_and(|program| names_program(command, program));
        let completing = index > 0
            && is_output_failure_line(lines[index - 1].0)
            && completes_failure_line(lines[index - 1].0, line);
        if errors.len() < MAX_ERROR_LINES
            && (failure || completing || states_error(line))
            && chosen.insert(clipped(line, MAX_LINE_BYTES))
        {
            errors.push(index);
        }
    }
    let (evidence, exit_code) = if flagged || code.is_some() {
        (Evidence::ToolStatus, code)
    } else if let Some(printed) = printed_status(command, &lines) {
        (Evidence::PrintedStatus, Some(printed))
    } else if missing {
        (Evidence::ShellReport, None)
    } else if fails && hides_status(command) {
        (Evidence::FailureLines, None)
    } else {
        return None;
    };
    let mut closing: Vec<usize> = (0..lines.len())
        .rev()
        .filter(|index| shown(lines[*index].0))
        .take(MAX_CLOSING_LINES)
        .collect();
    closing.reverse();
    let mut kept: Vec<(Kept, Range<usize>)> = Vec::new();
    let picked = errors
        .into_iter()
        .map(|index| (index, false))
        .chain(closing.into_iter().map(|index| (index, true)));
    for (index, closing) in picked {
        let (line, span) = &lines[index];
        let text = clipped(line, MAX_LINE_BYTES);
        // A closing line whose text an earlier chosen line holds marks that
        // line, so each text stands once.
        match kept.iter_mut().find(|(kept, _)| kept.text == text) {
            Some((earlier, _)) => earlier.closing |= closing,
            None => kept.push((Kept { text, closing }, span.clone())),
        }
    }
    Some(Reported {
        evidence,
        exit_code,
        lines: kept,
    })
}

/// The bytes of `text` that hold the redactor's findings, by source
/// capture's check of a tool span ([`kernel::exact_text_refusal`]): an
/// empty list for text the redactor passes whole, and `None` when its scan
/// does not finish.
fn findings_in(text: &str) -> Option<Vec<Range<usize>>> {
    match kernel::exact_text_refusal(text) {
        None => Some(Vec::new()),
        Some(kernel::ArtifactErrorKind::ExactBytesRewritten) => {
            let found = context_core::redaction::redact_windowed_durable_text(text, usize::MAX);
            let ranges = found.ok()?.detections.into_iter();
            Some(
                ranges
                    .map(|finding| finding.offset..finding.offset + finding.length)
                    .collect(),
            )
        }
        Some(_) => None,
    }
}

/// The first path and the first error name among the chosen lines that
/// `clear` passes, read from the lines as the output holds them.
fn named_in(
    lines: &[(Kept, Range<usize>)],
    text: &str,
    clear: impl Fn(&Range<usize>) -> bool,
) -> (Option<String>, Option<String>) {
    let named = |name: fn(&str) -> Option<&str>| {
        lines
            .iter()
            .filter(|(_, span)| clear(span))
            .find_map(|(_, span)| name(&text[span.clone()]))
            .map(str::to_owned)
    };
    (named(named_path), named(named_error))
}

/// The failure a record states, once the redactor has judged the output
/// `text` that `reported` was read from. The rule is source capture's rule
/// for a tool span: text in which the redactor finds nothing is kept as it
/// is, and text with a finding is withheld whole. With no finding the
/// chosen lines stand in the record. With a finding the record holds no
/// line, and the path, the error name, and the exit code come from lines
/// that hold no finding; when the scan does not finish, no text of the
/// output reaches the record. `None` for a failure that rests on an exit
/// code alone whose line holds a finding or was left unscanned: the result
/// then changes no record. A result the harness marks stays a failure
/// without that code.
fn screened(reported: Reported, text: &str, flagged: bool) -> Option<Failure> {
    let findings = findings_in(text);
    let clear = |span: &Range<usize>| {
        findings.as_ref().is_some_and(|findings| {
            findings
                .iter()
                .all(|finding| finding.end <= span.start || span.end <= finding.start)
        })
    };
    let exit_code = match &reported.exit_code {
        Some((code, span)) if clear(span) => Some(*code),
        Some(_) if flagged && reported.evidence == Evidence::ToolStatus => None,
        Some(_) => return None,
        None => None,
    };
    let (path, error) = named_in(&reported.lines, text, clear);
    let excerpt = match findings.as_deref() {
        Some([]) => Excerpt::Kept(reported.lines.into_iter().map(|(line, _)| line).collect()),
        Some(_) => Excerpt::Finding,
        None => Excerpt::Unscanned,
    };
    Some(Failure {
        evidence: reported.evidence,
        exit_code,
        path,
        error,
        excerpt,
    })
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
/// can order two failures of one second. `None` for a time outside the
/// years the calendar renders.
fn utc_time(at_ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(at_ms)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// The record's text within [`MAX_SUMMARY_BYTES`], with room for
/// [`RESOLVED_NOTE`]: the time of the result and what it states, the
/// command's name, the path and the error the output names, and the
/// output's chosen lines, each behind [`QUOTE_MARK`], or the sentence that
/// says why they are withheld and where the output stands. The time stands
/// in the first line, so a reader of a search result that is cut still
/// orders the failures of a project. The limits of the parts leave room for
/// the first two lines and for the closing lines or the withheld sentence
/// whole. The path and the error stand in the text when they fit beside
/// those, and the lines that state an error fill the bytes that remain; the
/// rationale holds the path and the error in every case.
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
    let mut summary = format!(
        "Tool failure recorded from the result of {} at {}: {stated}.\nCommand: {}",
        unit.tool,
        utc_time(unit.at_ms).unwrap_or_default(),
        unit.command.shown
    );
    let limit = MAX_SUMMARY_BYTES - RESOLVED_NOTE.len();
    let cost = |line: &Kept| 1 + QUOTE_MARK.len() + line.text.len();
    let (lines, ending) = match &failure.excerpt {
        Excerpt::Kept(lines) => (lines.as_slice(), "\nOutput lines:".to_owned()),
        withheld => {
            let reason = match withheld {
                Excerpt::Unscanned => "the redactor's scan of the output did not finish",
                _ => "the redactor found credential-shaped content in the output",
            };
            let sentence = format!(
                "\nOutput text withheld: {reason}. The whole output stands in the result of tool call {} of session {} while that session's history holds it.",
                clipped(&unit.call, MAX_ID_BYTES),
                clipped(session, MAX_ID_BYTES),
            );
            (&[][..], sentence)
        }
    };
    let mut reserved: usize = ending.len()
        + lines
            .iter()
            .filter(|line| line.closing)
            .map(cost)
            .sum::<usize>();
    for (label, name) in [("Failing path", &failure.path), ("Error", &failure.error)] {
        let line = name.as_ref().map(|name| format!("\n{label}: {name}"));
        if let Some(line) = line.filter(|line| summary.len() + line.len() + reserved <= limit) {
            summary.push_str(&line);
        }
    }
    summary.push_str(&ending);
    reserved -= ending.len();
    for line in lines {
        if line.closing {
            reserved -= cost(line);
        }
        if summary.len() + cost(line) + reserved <= limit {
            summary.push('\n');
            summary.push_str(QUOTE_MARK);
            summary.push_str(&line.text);
        }
    }
    summary
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
    /// Cancelled when the daemon shuts down. The task then ends before its
    /// next result and its next wait for the Kernel.
    cancel: CancellationToken,
}

/// Holds a session's place among the checkpoints that run; dropping it, on
/// any path, admits the session's next checkpoint.
struct MemoPin {
    memo: Arc<Mutex<FailureMemo>>,
    session: String,
    epoch: u64,
}

impl Drop for MemoPin {
    fn drop(&mut self) {
        let mut memo = self.memo.lock().expect("failure memo mutex");
        memo.release(&self.session, self.epoch);
    }
}

/// The id that names one execution of a tool call: Pi's native id, which
/// the Pi codec keeps beside the block when the block's own id is the
/// native id's first part, else the block's id.
fn execution_id<'a>(block: &'a WireBlock, id: &'a str) -> &'a str {
    block
        .provider_extras
        .get("pi")
        .and_then(|extras| extras.get("nativeToolCallId"))
        .and_then(Value::as_str)
        .unwrap_or(id)
}

impl HandlerCore {
    /// Copies of the command results of the window that this session's
    /// memo has not judged, for a primary Pi session whose configuration
    /// permits memory capture and consents to source capture: a record is a
    /// durable copy of part of a tool result, which source capture retains
    /// under that consent. A result under an execution id that two tool
    /// calls of the window share is left out, since the id names neither
    /// call alone. The pass considers the last [`MEMO_CALLS_PER_SESSION`]
    /// command results of the window and hands off the earliest
    /// [`MAX_RESULTS_PER_PASS`] unjudged ones whose commands and outputs
    /// fit [`MAX_HANDOFF_BYTES`] together; each is copied within
    /// [`MAX_OUTPUT_BYTES`], and a result that can change no record (see
    /// [`Input::of`]) is handed off by name alone and settled unread. The
    /// task holds no message of the pass. One checkpoint of a session runs
    /// at a time, and [`MAX_CHECKPOINTS`] over all sessions. Returns `None`
    /// when the window holds no unjudged result, so the caller spawns no
    /// task.
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
        // One command result: its execution id, the call's tool and input,
        // the result's output, and the result's creation time.
        type Candidate<'a> = (&'a str, &'a str, &'a Value, &'a OutputKind, Option<i64>);
        let mut calls: HashMap<&str, (&str, &Value)> = HashMap::new();
        let (mut named, mut shared) = (HashSet::new(), HashSet::new());
        let mut considered: VecDeque<Candidate<'_>> = VecDeque::new();
        for message in &request.messages {
            for block in message.ck.content() {
                match block.kind() {
                    BlockKind::ToolCall {
                        id, name, input, ..
                    } => {
                        let id = execution_id(block, id);
                        if !named.insert(id) {
                            shared.insert(id);
                        }
                        if command_field(input).is_some() {
                            calls.insert(id, (name.as_str(), input));
                        }
                    }
                    BlockKind::ToolResult { id, output, .. } => {
                        let id = execution_id(block, id);
                        let Some(&(tool, input)) = calls.get(id) else {
                            continue;
                        };
                        let at_ms = message.ck.meta.created_at_ms;
                        considered.push_back((id, tool, input, &output.kind, at_ms));
                        if considered.len() > MEMO_CALLS_PER_SESSION {
                            considered.pop_front();
                        }
                    }
                    _ => {}
                }
            }
        }
        considered.retain(|(id, ..)| !shared.contains(id));
        let epoch = {
            let mut memo = self.failure_memo.lock().expect("failure memo mutex");
            if memo.busy(&binding.session) {
                return None;
            }
            considered.retain(|(id, ..)| !memo.has_seen(&binding.session, call_digest(scope, id)));
            if considered.is_empty() {
                return None;
            }
            memo.pin(&binding.session)
        };
        let pin = MemoPin {
            memo: Arc::clone(&self.failure_memo),
            session: binding.session.clone(),
            epoch,
        };
        let mut pending = Vec::new();
        let mut copied = 0;
        for (id, tool, input, output, at_ms) in considered.into_iter().take(MAX_RESULTS_PER_PASS) {
            let input = Input::of(id, tool, input, output, at_ms);
            let bytes = input.as_ref().map_or(0, Input::bytes);
            if copied + bytes > MAX_HANDOFF_BYTES {
                break;
            }
            copied += bytes;
            pending.push(Pending {
                digest: call_digest(scope, id),
                input,
            });
        }
        Some(FailureCheckpoint {
            kernel: Arc::clone(&self.kernel),
            commit_gate: Arc::clone(&self.capture_commit_gate),
            project: binding.kernel_project.clone(),
            pin,
            pending,
            cancel: self.cancel.clone(),
        })
    }
}

impl Input {
    /// The copied inputs of one command result, or `None` for a result that
    /// changes no record whatever it holds: one that is no text or whose
    /// text or command line passes [`MAX_OUTPUT_BYTES`] (see [`text_of`],
    /// [`command_of`]), and one whose message carries no creation time or a
    /// time outside the years the calendar renders, which has no place
    /// among the command's runs.
    fn of(
        id: &str,
        tool: &str,
        input: &Value,
        output: &OutputKind,
        at_ms: Option<i64>,
    ) -> Option<Self> {
        let at_ms = at_ms.filter(|at_ms| utc_time(*at_ms).is_some())?;
        let command = command_of(input)?;
        let (text, flagged) = text_of(output)?;
        Some(Self {
            call: id.to_owned(),
            tool: tool.to_owned(),
            command,
            text,
            flagged,
            at_ms,
        })
    }

    /// The bytes this copy holds.
    fn bytes(&self) -> usize {
        self.call.len() + self.tool.len() + self.command.len() + self.text.len()
    }

    /// Reads the command and the result's outcome. `None` for a result that
    /// changes no record: one the harness refused or ended itself, one
    /// whose tool name or call id the redactor would change or whose
    /// command line fails the redactor's scan, and a failure that rests on
    /// an exit code alone whose line holds a finding (see [`screened`]).
    /// The tool's name and the call's id are judged whole, before they are
    /// cut for display.
    fn observe(&self, scope: &str) -> Option<Observed> {
        let passes = |text: &str| kernel::exact_text_refusal(text).is_none();
        if harness_decided(&self.text, self.flagged) || !passes(&self.tool) || !passes(&self.call) {
            return None;
        }
        let command = command_named(scope, &self.command)?;
        let outcome = match outcome_of(&self.command, &self.text, self.flagged) {
            None => Outcome::Passed,
            Some(reported) => Outcome::Failed(screened(reported, &self.text, self.flagged)?),
        };
        Some(Observed {
            call: self.call.clone(),
            tool: tool_shown(&self.tool),
            command,
            at_ms: self.at_ms,
            outcome,
        })
    }
}

/// What judging one command result did.
enum Judged {
    Wrote,
    Nothing,
    /// The result writes nothing for a reason an operator can act on.
    Left(&'static str),
}

impl FailureCheckpoint {
    /// Text scans, Kernel reads, and commits run on the blocking pool, and
    /// the task ends once they have. A daemon shutdown raises the task's
    /// interrupt, which ends the pass before its next result and ends a
    /// wait for the Kernel that is under way. Returns the number of rows
    /// written.
    pub(crate) async fn run(self) -> usize {
        let interrupt = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let raised = Arc::clone(&interrupt);
        let mut join = tokio::task::spawn_blocking(move || self.record(&raised));
        let joined = tokio::select! {
            joined = &mut join => joined,
            () = cancel.cancelled() => {
                interrupt.store(true, Ordering::Release);
                join.await
            }
        };
        match joined {
            Ok(written) => written,
            Err(error) => match error.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                Err(_) => 0,
            },
        }
    }

    /// Judges the results in window order. Each result is read ahead of the
    /// capture publication gate and judged under it, while the session's
    /// memo entry is the one this checkpoint pinned; the memo learns a call
    /// once its judgment has ended. A Kernel that is busy or failing, and a
    /// row another writer changed under the judgment, end the pass with the
    /// call unlearned, so a later pass whose window still carries the call
    /// and its result judges it and the results after it again, in the
    /// order they ran. Any other refusal of the Kernel settles the call:
    /// the same result would meet it again. A panic under the gate leaves
    /// the gate usable: the guard is released before the panic goes on. A
    /// refusal is reported on stderr after the gate is released.
    fn record(self, interrupt: &Arc<AtomicBool>) -> usize {
        let Self {
            kernel,
            commit_gate,
            project,
            pin,
            pending,
            cancel: _,
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
        // The session's id stands in a record's text and rationale, so the
        // redactor judges it whole, once.
        let session_passes = kernel::exact_text_refusal(&pin.session).is_none();
        let stopped = || interrupt.load(Ordering::Acquire);
        let mut written = 0;
        for result in &pending {
            if stopped() {
                break;
            }
            let unit = result
                .input
                .as_ref()
                .filter(|_| session_passes)
                .and_then(|input| input.observe(writer.scope));
            // The gate orders the Kernel reads, the commit, and the memo
            // update of every checkpoint, and session deletion takes it too.
            let gate = commit_gate.lock().expect("capture publication mutex");
            let judged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let memo = || pin.memo.lock().expect("failure memo mutex");
                if stopped() || !memo().current(&pin.session, pin.epoch) {
                    return None;
                }
                let budget =
                    EvalBudget::new(Some(Instant::now() + COMMIT_WAIT), Arc::clone(interrupt));
                let judged = match &unit {
                    Some(unit) => writer.judge(unit, &budget),
                    None => Ok(Judged::Nothing),
                };
                use kernel::KernelError::{Busy, Deadline, Fault, FenceLost, Held, Io, NotFound};
                if !matches!(
                    judged,
                    Err(Deadline | Busy | Held | Io | FenceLost | Fault | NotFound)
                ) {
                    memo().learn(&pin.session, pin.epoch, result.digest);
                }
                Some(judged)
            }));
            drop(gate);
            match judged {
                Err(panic) => std::panic::resume_unwind(panic),
                Ok(None) => break,
                Ok(Some(Ok(Judged::Wrote))) => written += 1,
                Ok(Some(Ok(Judged::Nothing))) => {}
                Ok(Some(Ok(Judged::Left(reason)))) => {
                    eprintln!("daemon: tool failure record left unwritten: {reason}");
                }
                Ok(Some(Err(error))) => {
                    eprintln!("daemon: tool failure record left unwritten: {error}");
                    use kernel::KernelError::{
                        Busy, Deadline, Fault, FenceLost, Held, Io, NotFound,
                    };
                    if matches!(
                        error,
                        Deadline | Busy | Held | Io | FenceLost | Fault | NotFound
                    ) {
                        break;
                    }
                }
            }
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
    let text = match failure.excerpt {
        Excerpt::Kept(_) => "kept",
        Excerpt::Finding | Excerpt::Unscanned => "withheld",
    };
    for (name, value) in [
        ("source", json!(FAILURE_SOURCE_ID)),
        ("session", json!(session)),
        ("call", json!(unit.call)),
        ("tool", json!(unit.tool)),
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
        ("path", failure.path.clone().map(Value::from)),
        ("error", failure.error.clone().map(Value::from)),
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
    stored: &kernel::DecisionPayload,
    mut rationale: serde_json::Map<String, Value>,
) -> kernel::DecisionPayload {
    let line = stored.summary.find('\n').unwrap_or(stored.summary.len());
    let (stated, rest) = stored.summary.split_at(line);
    for (name, value) in [
        ("state", json!("resolved")),
        ("resolved_session", json!(session)),
        ("resolved_call", json!(unit.call)),
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
    /// Writes what one command result calls for. The caller holds the
    /// capture publication gate, so no other checkpoint writes between the
    /// reads and the commit. Every wait for a Kernel connection ends at the
    /// budget's deadline.
    ///
    /// A call whose commit the Kernel's log holds writes nothing: the log's
    /// receipt under this recorder's producer name is the proof that the
    /// call's record was written, whatever became of the row. The command's
    /// record is the newest decision of its lineage, live or retired, when
    /// that decision is a failure record of this project that states its
    /// run; a lineage whose newest decision is anything else is left as it
    /// is. A run follows a record when its result's creation time is
    /// greater than the time of the record's run. A failure writes the first
    /// row of a lineage, a row after a retired record that it follows, and
    /// a row in place of a live record that it follows. A pass rewrites a
    /// live, unresolved record that it follows as resolved. Either writes
    /// while the session has written fewer than
    /// [`MAX_RECORDS_PER_SESSION`] records by the Kernel's commit log.
    fn judge(&self, unit: &Observed, budget: &EvalBudget) -> Result<Judged, kernel::KernelError> {
        let id = record_id(self.scope, self.session, &unit.call);
        let (session_key, _) = session_keys(self.session);
        let key = self
            .project
            .operation_key(FAILURE_SOURCE_ID, &format!("{session_key}{id}"))
            .ok_or(kernel::KernelError::InvalidInput)?;
        // A record id has one length, so the keys from this one to its
        // successor in byte order are this key alone.
        let committed = self.kernel.commit_count_in_key_range_within_budget(
            budget,
            FAILURE_SOURCE_ID,
            (&key, &format!("{key}~")),
            1,
        )?;
        if committed > 0 {
            return Ok(Judged::Nothing);
        }
        let lineage = format!("{FAILURE_SOURCE_ID}:{}", unit.command.digest);
        let (tip, newest) = self.kernel.newest_decisions_on_lineage_within_budget(
            budget,
            CAPTURE_SOURCE_KIND,
            &lineage,
        )?;
        let head = match newest.as_slice() {
            [] => None,
            [first, second] if first.source_revision == second.source_revision => {
                return Ok(Judged::Left(
                    "two decisions hold the newest revision of the command's lineage",
                ));
            }
            [first, ..] => Some(first),
        };
        let (revision, stored) = match head {
            None => (1, None),
            Some(row) => {
                let unread = Judged::Left(
                    "the newest decision of the command's lineage is no record of this recorder",
                );
                let Some(revision) = row.source_revision.checked_add(1) else {
                    return Ok(unread);
                };
                if row.superseded_by.is_some() {
                    return Ok(unread);
                }
                // A retired record is read as it stood before its
                // retirement.
                let as_of = row
                    .invalidated_commit_seq
                    .map_or(tip, |retired| retired - 1);
                let ids = std::slice::from_ref(&row.object_id);
                let decision = self
                    .kernel
                    .decisions_for_objects_as_of_within_budget(ids, as_of, budget)?
                    .pop()
                    .ok_or(kernel::KernelError::NotFound)?;
                let owned = decision.decision_kind == FAILURE_CATEGORY
                    && decision.scope_id.as_deref() == Some(self.project.scope_id().as_str());
                let Some(stored) = Stored::read(&decision.payload.rationale).filter(|_| owned)
                else {
                    return Ok(unread);
                };
                if unit.at_ms <= stored.at_ms {
                    return Ok(Judged::Nothing);
                }
                (revision, Some((row, decision.payload, stored)))
            }
        };
        let live = stored
            .as_ref()
            .filter(|(row, ..)| row.invalidated_commit_seq.is_none());
        let payload = match (&unit.outcome, live) {
            (Outcome::Failed(failure), _) => failed_record(self.session, unit, failure),
            (Outcome::Passed, Some((_, payload, stored))) if !stored.resolved => {
                resolved_record(self.session, unit, payload, stored.rationale.clone())
            }
            (Outcome::Passed, _) => return Ok(Judged::Nothing),
        };
        let (from, to) = session_keys(self.session);
        let bound = |part: &str| {
            self.project
                .operation_key(FAILURE_SOURCE_ID, part)
                .ok_or(kernel::KernelError::InvalidInput)
        };
        // The commits of this recorder that the log holds under the
        // session's keys, counted up to the session's bound.
        let written = self.kernel.commit_count_in_key_range_within_budget(
            budget,
            FAILURE_SOURCE_ID,
            (&bound(&from)?, &bound(&to)?),
            MAX_RECORDS_PER_SESSION,
        )?;
        if written >= MAX_RECORDS_PER_SESSION {
            return Ok(Judged::Nothing);
        }
        let target = live.map(|(row, ..)| row.object_id.as_str());
        self.commit((id, key), (&lineage, revision), target, payload, budget)
    }

    /// Commits the record `id` under the operation key `key` at `revision`
    /// of `lineage`, in place of the live row `target` when one is given, as
    /// a labeled row. The Kernel stores the text and the rationale through
    /// the durable-text redactor; a payload the redactor would change writes
    /// no record, so the stored row is the row composed here. A commit the
    /// Kernel answers from an earlier receipt wrote nothing now.
    fn commit(
        &self,
        (id, key): (String, String),
        (lineage, revision): (&str, i64),
        target: Option<&str>,
        payload: kernel::DecisionPayload,
        budget: &EvalBudget,
    ) -> Result<Judged, kernel::KernelError> {
        let project = self.project;
        let clean = [&payload.summary, &payload.rationale]
            .into_iter()
            .all(|text| kernel::exact_text_refusal(text).is_none());
        if !clean {
            return Ok(Judged::Left("the redactor changes the record's text"));
        }
        let intent = kernel::CommitIntent {
            producer: FAILURE_SOURCE_ID.into(),
            operation_key: key,
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
        Ok(match receipt.replayed {
            true => Judged::Nothing,
            false => Judged::Wrote,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCOPE: &str = "project";

    /// The ground, the exit code, and the chosen lines of a failed text
    /// result of `command`, as the output states them.
    fn failed(
        command: &str,
        text: &str,
        flagged: bool,
    ) -> Option<(Evidence, Option<i64>, Vec<String>)> {
        outcome_of(command, text, flagged).map(|reported| {
            (
                reported.evidence,
                reported.exit_code.map(|(code, _)| code),
                reported
                    .lines
                    .into_iter()
                    .map(|(line, _)| line.text)
                    .collect(),
            )
        })
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

    /// A provider token that is one plain word, joined at run time.
    fn bare_token() -> String {
        [
            "sk-",
            "ant-",
            "api03-",
            "abcdefghijklmnopqrstuvwxyz",
            "ABCDEFGH12345678",
        ]
        .concat()
    }

    fn input(command: &str, text: &str, flagged: bool) -> Input {
        Input {
            call: "call-1".into(),
            tool: "bash".into(),
            command: command.into(),
            text: text.into(),
            flagged,
            at_ms: 1_000,
        }
    }

    /// The failure a text result of `command` records, as the task reads
    /// it: `None` for a pass and for a result that changes no record.
    fn recorded(command: &str, text: &str, flagged: bool) -> Option<Failure> {
        match input(command, text, flagged).observe(SCOPE)?.outcome {
            Outcome::Passed => None,
            Outcome::Failed(failure) => Some(failure),
        }
    }

    #[test]
    fn a_command_is_read_from_the_call_within_its_bytes() {
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
        // A command line of the bound is read, and one byte more is left
        // out, for a string and for an array with its joining spaces. A
        // `command` past the bound falls back to no `cmd`.
        let at_bound = "x".repeat(MAX_OUTPUT_BYTES);
        assert!(command_of(&json!({ "command": at_bound })).is_some());
        let past = format!("{at_bound}x");
        assert_eq!(command_of(&json!({ "command": past, "cmd": "ls" })), None);
        let half = "x".repeat(MAX_OUTPUT_BYTES / 2);
        let words = json!({ "command": [half, &half[1..]] });
        assert_eq!(
            command_of(&words).map(|line| line.len()),
            Some(MAX_OUTPUT_BYTES)
        );
        assert_eq!(command_of(&json!({ "command": [half, half] })), None);
    }

    #[test]
    fn a_result_s_text_is_read_within_its_bytes_and_a_value_is_left_out() {
        let at_bound = "x".repeat(MAX_OUTPUT_BYTES);
        for (output, flagged) in [
            (
                OutputKind::Text {
                    text: at_bound.clone(),
                },
                false,
            ),
            (
                OutputKind::ErrorText {
                    text: at_bound.clone(),
                },
                true,
            ),
        ] {
            assert_eq!(text_of(&output), Some((at_bound.clone(), flagged)));
        }
        for output in [
            OutputKind::Text {
                text: format!("{at_bound}x"),
            },
            OutputKind::ErrorText {
                text: format!("{at_bound}x"),
            },
            OutputKind::Json {
                value: json!({ "exit_code": 1 }),
            },
            OutputKind::ErrorJson {
                value: json!("boom"),
            },
        ] {
            assert_eq!(text_of(&output), None);
        }
        // A text that is one JSON object is text: its exit field decides
        // nothing, since a passing command can print one.
        let object = json!({ "exitCode": 137, "reason": "OOMKilled" }).to_string();
        assert_eq!(
            outcome_of("kubectl get pod api -o json", &object, false),
            None
        );
    }

    #[test]
    fn a_command_is_named_by_its_program_and_first_plain_operand() {
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
            ("\"my tool\" run", LEFT_OUT),
            ("(cd api && make)", LEFT_OUT),
            ("$TOOL run", LEFT_OUT),
            // A leading assignment or directory step whose word holds a
            // command substitution stays, so no word inside the
            // substitution is taken for the program.
            ("VERSION=$(git describe --tags) make release", LEFT_OUT),
            ("VERSION=`git describe --tags` make release", LEFT_OUT),
            ("cd $(a && b c) && make", "cd [arguments left out]"),
            ("X=$(pwd) make", LEFT_OUT),
        ] {
            assert_eq!(named(command).shown, shown, "{command}");
        }
        let long = "x".repeat(MAX_WORD_BYTES + 1);
        assert_eq!(named(&long).shown, LEFT_OUT);
        assert_eq!(
            named(&format!("make {long}")).shown,
            "make [arguments left out]"
        );
        // An operand the redactor rewrites is no plain word: the raw token
        // is one, and the record shows the program alone.
        let token = bare_token();
        assert!(plain_word(&token));
        assert!(kernel::exact_text_refusal(&format!("deploy {token}")).is_some());
        assert_eq!(
            named(&format!("deploy {token}")).shown,
            "deploy [arguments left out]"
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
            "cargo\ttest   -p api  ",
        ] {
            assert_eq!(digest(same), plain, "{same}");
        }
        for other in [
            "cargo test -p web",
            "cargo test",
            "cargo test -p api && cd ..",
            "cd services || cargo test -p api",
            "cd $(pwd) && cargo test -p api",
        ] {
            assert_ne!(digest(other), plain, "{other}");
        }
        assert_ne!(
            command_named("another project", "cargo test -p api")
                .unwrap()
                .digest,
            plain
        );
        // Whitespace inside quotes and behind a backslash is an argument's
        // own text, and whitespace between words folds.
        assert_ne!(digest("grep 'a  b' log"), digest("grep 'a b' log"));
        assert_ne!(digest("grep a\\  b"), digest("grep a\\ b"));
        assert_eq!(digest("grep  'a  b'   log"), digest("grep 'a  b' log"));
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
        // The word reader stops at the first word end: many leading steps
        // are read once each.
        let steps = "A=1 cd a; ".repeat(2_000);
        assert_eq!(named(&format!("{steps}make")).shown, "make");
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
            ("ELIFECYCLE\u{2009} Command failed with exit code 1.", 1),
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
        let code = |text: &str| failed("make", text, false).map(|failure| failure.1);
        assert_eq!(
            code("Exit code: 4\nfirst\nsecond\nthird\nfourth"),
            Some(Some(4))
        );
        assert_eq!(code("one\nexit code 5\ntwo\nthree"), Some(Some(5)));
        assert_eq!(code("one\nexit code 5\ntwo\nthree\nfour"), None);
        assert_eq!(code("exit code 6\nwork\nexit code 7"), Some(Some(7)));
        assert_eq!(code("exit code 6\nwork\nexit code 0"), None);
        // The first line counts when it ends within the output's first
        // 1,024 bytes, blank lines and leading whitespace included.
        let pad = |bytes: usize| format!("{}exit code 8\nrest\nof\nthe\noutput", " ".repeat(bytes));
        let within = MAX_FIRST_LINE_BYTES - "exit code 8".len();
        assert_eq!(code(&pad(within)), Some(Some(8)));
        assert_eq!(code(&pad(within + 1)), None);
    }

    #[test]
    fn a_nonzero_status_the_command_line_printed_is_a_failure() {
        let printed = |command: &str, text: &str| {
            failed(command, text, false).map(|failure| (failure.0, failure.1))
        };
        let status = |code| Some((Evidence::PrintedStatus, Some(code)));
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
            ("make check; echo \"rc=$?\" 1>&2", "rc=101", 101),
            ("make check; echo \"rc=$?\" 1>&2", "rc=1", 1),
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
            // A parameter in single quotes or behind a backslash is text
            // the echo prints as written, beside the one that expands.
            ("false; echo '$?' \"$?\"", "$? 1", 1),
            ("false; echo \"\\$? is $?\"", "$? is 7", 7),
            // Each echo that prints one status is read, and the greatest
            // status decides: an echo that printed nothing and an earlier
            // zero hide no later status.
            (
                "false && echo pre=$?; make check; echo result=$?",
                "result=2",
                2,
            ),
            (
                "cargo build; echo \"build=$?\"; cargo test; echo \"test=$?\"",
                "build=0\ntest=101",
                101,
            ),
            // A malformed parameter is the echo's text, and the next echo
            // is read.
            (
                "echo \"${PIPESTATUS[\"; make; echo \"rc=$?\"",
                "${PIPESTATUS[\nrc=3",
                3,
            ),
        ] {
            assert_eq!(printed(command, text), status(code), "{command}");
        }
        // The command line asked for no status, the shell left the
        // parameter as written, the output states none in the echo's
        // words, or the echo prints two statuses: the line is ordinary
        // output.
        for (command, text) in [
            ("make check", "exit=2"),
            ("make check; echo 'exit=$?'", "exit=2"),
            ("make check; echo \"exit=\\$?\"", "exit=2"),
            ("make check; echo \"exit=$?\"", "exit=two"),
            ("make check; echo \"exit=$?\"", "the exit=2 line"),
            ("make check; echo $?", "2\ndone"),
            ("make check; echo \"$? $?\"", "2 2"),
            ("make check; printf 'exit=%s' $?", "exit=2"),
            // The space beside the parameter is part of the echo's words.
            ("make check; echo \"status $?\"", "status 0\nstatus3"),
            ("make check; echo \"$? done\"", "3done"),
        ] {
            assert_eq!(printed(command, text), None, "{command}: {text}");
        }
        assert_eq!(
            printed("make check; echo \"status $?\"", "status 3"),
            status(3)
        );
        // A printed status of zero decides nothing: the grounds below read
        // the command line without its echo.
        const ERROR: &str = "error: could not compile `api`";
        assert_eq!(
            printed("make check; echo \"exit=$?\"", &format!("{ERROR}\nexit=0")),
            None,
            "the echo reports the one command before it"
        );
        assert_eq!(
            printed(
                "make check | tail -n 40; echo \"exit=$?\"",
                &format!("{ERROR}\nexit=0")
            ),
            Some((Evidence::FailureLines, None)),
            "the echo reports the last command of the pipeline"
        );
    }

    #[test]
    fn a_result_the_tool_reports_as_passed_fails_by_the_shell_s_report_or_behind_a_hidden_status() {
        use Evidence::{FailureLines, ShellReport};
        const ERROR: &str = "error: could not compile `api`";
        let ground = |command: &str, text: &str| {
            failed(command, text, false).map(|failure| (failure.0, failure.1))
        };
        // The shell's own report of a program of the command line.
        for (command, text) in [
            ("pnpm typecheck", "bash: line 1: pnpm: command not found"),
            (
                "pnpm typecheck 2>&1 | tee check.log",
                "/usr/bin/bash: line 1: pnpm: command not found",
            ),
            ("cd api && pnpm build", "sh: pnpm: command not found"),
            ("pnpm typecheck", "zsh: command not found: pnpm"),
        ] {
            assert_eq!(ground(command, text), Some((ShellReport, None)), "{text}");
        }
        // The words in prose, a report of a program the command line does
        // not name, and a line without the shell's name report nothing.
        for (command, text) in [
            (
                "grep -rn \"command not found\" ci.log",
                "ci.log:4: step failed: command not found",
            ),
            (
                "cat docs/troubleshooting.md",
                "If you see `command not found`, install pnpm.",
            ),
            (
                "grep -rn pnpm ci.log | head",
                "bash: line 1: yarn: command not found",
            ),
            ("pnpm typecheck", "pnpm: command not found"),
            ("pnpm typecheck", "bash: pnpm typecheck: command not found"),
        ] {
            assert_eq!(ground(command, text), None, "{command}: {text}");
        }
        // A failure line is a failure where a pipe, `||`, or `;` stands
        // between two commands of the line.
        for command in [
            "make check | tail -n 40",
            "make check 2>&1 | tee build.log",
            "make check || true",
            "make check; ls target",
            "make check; echo done;",
            "cd api && make check | head -5",
        ] {
            assert_eq!(
                ground(command, ERROR),
                Some((FailureLines, None)),
                "{command}"
            );
        }
        // A single command, an `&&` chain, a quoted or escaped `|` and
        // `;`, and a `;` that ends the line hide none: the tool's status is
        // the command's own.
        for command in [
            "make check",
            "grep -rn error: logs/",
            "grep -rn error: logs/;",
            "grep -rn error: logs/ ; \n",
            "cd api && make check",
            "make check 2>&1",
            "grep -E 'error|warning' build.log",
            "echo \"a; b\"",
            "find . -name x -exec cat {} \\;",
            "make check; echo \"exit=$?\"",
        ] {
            assert_eq!(ground(command, ERROR), None, "{command}");
        }
        // Without a failure line a hidden status is a pass, and a stated
        // status of zero is the status of the line's last command.
        assert_eq!(
            ground(
                "make check | tail -n 40",
                "warning: unused import\nall targets built"
            ),
            None
        );
        assert_eq!(
            ground("make check | tail -n 40", &format!("{ERROR}\nExit code: 0")),
            Some((FailureLines, None))
        );
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
            assert!(harness_decided(text, true), "{text}");
            assert!(!harness_decided(text, false), "unmarked: {text}");
            assert!(input("make", text, true).observe(SCOPE).is_none(), "{text}");
        }
        for text in [
            "Command aborted\nand then more output",
            "Tool execution was blocked by policy",
            "Tool bash and zsh not found",
            "error: Validation failed for tool \"bash\"",
            "Command exited with code 1",
        ] {
            assert!(!harness_decided(text, true), "{text}");
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
        // with a control character or a Unicode line separator is left out.
        let long = format!(
            "error: {}\nbell \u{7} line\nerror: split\u{2028}line\nCommand exited with code 1",
            "é".repeat(200)
        );
        let (_, _, kept) = failed("make", &long, true).unwrap();
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept[0].len() <= MAX_LINE_BYTES && kept[0].ends_with(CUT_MARK));
        assert_eq!(kept[1], "Command exited with code 1");
        // A text the output repeats is chosen once, so a repeated error
        // leaves its slots to the distinct errors after it.
        let repeated = format!(
            "{}error: the cause\nnote one\nnote two\nexit status 1\nexit status 1",
            "error: build failed\n".repeat(5)
        );
        let (_, code, kept) = failed("make", &repeated, true).unwrap();
        assert_eq!(code, Some(1));
        assert_eq!(
            kept,
            ["error: build failed", "error: the cause", "exit status 1"]
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

    #[test]
    fn an_output_the_redactor_finds_a_credential_in_keeps_no_line() {
        let (assignment, value) = keyed_credential();
        let clean =
            "error[E0308]: mismatched types\n  --> src/deploy.rs:14:9\nCommand exited with code 1";
        let kept = recorded("make", clean, true).unwrap();
        assert_eq!(
            (kept.exit_code, kept.path.as_deref(), kept.error.as_deref()),
            (Some(1), Some("src/deploy.rs:14:9"), Some("E0308"))
        );
        let Excerpt::Kept(lines) = &kept.excerpt else {
            panic!("{kept:?}");
        };
        assert_eq!(lines.len(), 3);
        // A finding anywhere in the output withholds every line. The path,
        // the error name, and the exit code come from lines without a
        // finding.
        let found = recorded(
            "make",
            &format!("note: retry with {assignment}\n{clean}"),
            true,
        );
        assert_eq!(
            found,
            Some(Failure {
                evidence: Evidence::ToolStatus,
                exit_code: Some(1),
                path: Some("src/deploy.rs:14:9".into()),
                error: Some("E0308".into()),
                excerpt: Excerpt::Finding,
            })
        );
        // A path and an error name on the line of the finding stay out.
        let in_line =
            format!("ValueError: {assignment} at src/deploy.rs:14:9\nCommand exited with code 1");
        let found = recorded("make", &in_line, true).unwrap();
        assert!(!format!("{found:?}").contains(&value));
        assert_eq!(
            (found.exit_code, found.path, found.error, found.excerpt),
            (Some(1), None, None, Excerpt::Finding)
        );
        // A scan that does not finish keeps no text at all, and the exit
        // code with it: a key block longer than the redactor matches.
        let block = format!(
            "ValueError: bad input at src/deploy.rs:14:9\n-----BEGIN {kind}-----\n{body}-----END {kind}-----\nCommand exited with code 1",
            kind = ["RSA PRIVATE", " KEY"].concat(),
            body = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2\n".repeat(600),
        );
        assert!(block.len() <= MAX_OUTPUT_BYTES);
        assert_eq!(
            recorded("make", &block, true),
            Some(Failure {
                evidence: Evidence::ToolStatus,
                exit_code: None,
                path: None,
                error: None,
                excerpt: Excerpt::Unscanned,
            })
        );
        // An unmarked result whose one ground is that unscanned code
        // changes no record: it is no failure and no pass.
        assert!(input("make", &block, false).observe(SCOPE).is_none());
    }

    #[test]
    fn an_exit_code_whose_line_holds_a_finding_reaches_no_record() {
        // The command line prints its status as the end of a value under a
        // secret's name, so the redactor's finding holds the digits.
        let key = ["api_", "tok", "en"].concat();
        let command = format!("deploy; echo \"{key}=fixture-v$?\"");
        let line = format!("{key}=fixture-v7");
        assert!(
            kernel::exact_text_refusal(&line).is_some(),
            "the fixture is a finding"
        );
        let reported = outcome_of(&command, &line, false).unwrap();
        assert_eq!(
            (
                reported.evidence,
                reported.exit_code.as_ref().map(|(code, _)| *code)
            ),
            (Evidence::PrintedStatus, Some(7))
        );
        // The digits are the one ground of the failure: the result changes
        // no record.
        assert!(input(&command, &line, false).observe(SCOPE).is_none());
        // A marked result stays a failure and keeps no text of the line.
        let marked = recorded(&command, &line, true).unwrap();
        assert_eq!(
            (marked.evidence, marked.exit_code, &marked.excerpt),
            (Evidence::ToolStatus, None, &Excerpt::Finding)
        );
        // The positive control: a status line without a finding keeps its
        // code beside a finding on another line.
        let clean = recorded(
            "deploy",
            &format!("{line}\nCommand exited with code 3"),
            true,
        );
        assert_eq!(clean.unwrap().exit_code, Some(3));
    }

    #[test]
    fn a_tool_name_a_call_id_and_a_time_are_judged_whole_before_a_record_shows_them() {
        let (assignment, value) = keyed_credential();
        let failing = |edit: &dyn Fn(&mut Input)| {
            let mut input = input("make", "error: boom\nCommand exited with code 1", true);
            edit(&mut input);
            input.observe(SCOPE)
        };
        assert!(failing(&|_| {}).is_some(), "the positive control");
        // An id the redactor would change past the bytes a record shows
        // writes nothing: the id is judged before it is cut.
        let long_id = format!("{}{assignment}", "c".repeat(MAX_ID_BYTES));
        assert!(!clipped(&long_id, MAX_ID_BYTES).contains(&value));
        assert!(failing(&|input| input.call = long_id.clone()).is_none());
        let long_tool = format!("{} {assignment}", "t".repeat(MAX_TOOL_BYTES));
        assert!(!tool_shown(&long_tool).contains(&value));
        assert!(failing(&|input| input.tool = long_tool.clone()).is_none());
        // The tool's name is shown with its whitespace folded, within its
        // bytes, cut at a character boundary.
        for name in [
            "bash",
            "  run   shell ",
            &"é".repeat(40),
            &"word ".repeat(40),
        ] {
            assert_eq!(
                tool_shown(name),
                clipped(&collapsed(name), MAX_TOOL_BYTES),
                "{name}"
            );
        }
        // A result without a creation time, or with one the calendar does
        // not render, is copied for no judgment.
        let call = json!({ "command": "make" });
        let output = OutputKind::ErrorText {
            text: "error: boom".into(),
        };
        let of = |at_ms| Input::of("call-1", "bash", &call, &output, at_ms);
        assert!(of(Some(1_000)).is_some());
        for at_ms in [
            None,
            Some(i64::MAX),
            Some(8_700_000_000_000_000),
            Some(i64::MIN),
        ] {
            assert!(of(at_ms).is_none(), "{at_ms:?}");
        }
    }

    fn observed(command: &str) -> Observed {
        Observed {
            call: "call-1".into(),
            tool: "bash".into(),
            command: named(command),
            at_ms: 1_000,
            outcome: Outcome::Passed,
        }
    }

    fn failure(evidence: Evidence, exit_code: Option<i64>, excerpt: Excerpt) -> Failure {
        Failure {
            evidence,
            exit_code,
            path: Some("src/render.rs:9:5".into()),
            error: Some("E0308".into()),
            excerpt,
        }
    }

    #[test]
    fn the_record_states_the_outcome_the_command_and_the_lines_within_its_bytes() {
        let kept = |text: &str, closing: bool| Kept {
            text: text.into(),
            closing,
        };
        let lines = vec![kept("error: first", false), kept("the end", true)];
        let head = "Tool failure recorded from the result of bash at 1970-01-01T00:00:01.000Z: ";
        let tail = ".\nCommand: cargo test [arguments left out]\nFailing path: src/render.rs:9:5\nError: E0308\nOutput lines:\n> error: first\n> the end";
        let unit = observed("cargo test -p api");
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
            let failure = failure(evidence, code, Excerpt::Kept(lines.clone()));
            assert_eq!(
                summary_of(&unit, "s1", &failure),
                format!("{head}{stated}{tail}")
            );
        }
        // A command line that begins with no plain word is shown as left
        // out, in the text and in the rationale.
        let failure = failure(Evidence::ToolStatus, Some(1), Excerpt::Kept(lines));
        let quoted = observed("\"my tool\" run");
        let record = failed_record("s1", &quoted, &failure);
        assert!(
            record.summary.contains("\nCommand: [left out]\n"),
            "{}",
            record.summary
        );
        let rationale: Value = serde_json::from_str(&record.rationale).unwrap();
        assert_eq!(rationale["command"], LEFT_OUT);
    }

    /// The parts of a record at their byte limits leave the closing lines
    /// and the withheld sentence whole, in the failed text and in its
    /// resolved copy, and the path and the error give way first.
    #[test]
    fn a_record_at_the_limits_of_its_parts_keeps_its_ending_whole() {
        let long_word = "w".repeat(MAX_WORD_BYTES);
        let mut unit = observed(&format!("{long_word} {long_word} --more"));
        unit.tool = "t".repeat(MAX_TOOL_BYTES);
        unit.call = "c".repeat(MAX_ID_BYTES + 40);
        let session = "s".repeat(MAX_ID_BYTES + 40);
        let closing = |fill: char| Kept {
            text: fill.to_string().repeat(MAX_LINE_BYTES),
            closing: true,
        };
        let mut lines: Vec<Kept> = (0..MAX_ERROR_LINES)
            .map(|index| Kept {
                text: format!("error {index}: {}", "e".repeat(MAX_LINE_BYTES - 9)),
                closing: false,
            })
            .collect();
        lines.extend([closing('y'), closing('z')]);
        let token = "p".repeat(MAX_TOKEN_BYTES);
        let at_limits = |excerpt| Failure {
            evidence: Evidence::FailureLines,
            exit_code: None,
            path: Some(token.clone()),
            error: Some(token.clone()),
            excerpt,
        };
        let kept = at_limits(Excerpt::Kept(lines));
        let record = failed_record(&session, &unit, &kept);
        let ending = format!(
            "\n> {}\n> {}",
            "y".repeat(MAX_LINE_BYTES),
            "z".repeat(MAX_LINE_BYTES)
        );
        assert!(record.summary.ends_with(&ending), "{}", record.summary);
        assert!(record.summary.contains("\nOutput lines:\n"));
        for excerpt in [Excerpt::Finding, Excerpt::Unscanned] {
            let withheld = failed_record(&session, &unit, &at_limits(excerpt));
            let ids = format!(
                "tool call {}{CUT_MARK} of session {}{CUT_MARK} while that session's history holds it.",
                "c".repeat(MAX_ID_BYTES - CUT_MARK.len()),
                "s".repeat(MAX_ID_BYTES - CUT_MARK.len()),
            );
            assert!(withheld.summary.ends_with(&ids), "{}", withheld.summary);
            assert!(!withheld.summary.contains("Output lines:"));
            let resolved = resolved_record(
                &session,
                &unit,
                &withheld,
                Stored::read(&withheld.rationale).unwrap().rationale,
            );
            assert!(
                resolved.summary.ends_with(&ids) && resolved.summary.len() <= MAX_SUMMARY_BYTES
            );
            // The rationale holds the whole path, error, call, and session.
            let rationale: Value = serde_json::from_str(&resolved.rationale).unwrap();
            assert_eq!(
                (&rationale["path"], &rationale["error"]),
                (&json!(token), &json!(token))
            );
            assert_eq!(
                rationale["call"].as_str().map(str::len),
                Some(MAX_ID_BYTES + 40)
            );
            assert_eq!(rationale["session"], json!(session));
        }
        let stored = Stored::read(&record.rationale).unwrap().rationale;
        let resolved = resolved_record(&session, &unit, &record, stored);
        assert!(
            resolved.summary.len() <= MAX_SUMMARY_BYTES,
            "{}",
            resolved.summary.len()
        );
        assert!(resolved.summary.ends_with(&ending));
        assert!(
            resolved
                .summary
                .lines()
                .next()
                .unwrap()
                .ends_with(RESOLVED_NOTE.trim_start())
        );
    }

    /// A record is written when the redactor leaves its text and its
    /// rationale as composed. The names a record carries beside secret
    /// words (a program, a path, an error type) and the ids a provider
    /// gives a tool call and a session are ordinary text to the redactor,
    /// for a failed record and for its resolved copy.
    #[test]
    fn a_record_of_ordinary_names_and_ids_passes_the_redactor_as_composed() {
        let session = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let lines = vec![Kept {
            text: "Command exited with code 1".into(),
            closing: true,
        }];
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
                for excerpt in [Excerpt::Kept(lines.clone()), Excerpt::Finding] {
                    let mut unit = observed(command);
                    unit.call = call.into();
                    let failure = Failure {
                        evidence: Evidence::ToolStatus,
                        exit_code: Some(1),
                        path: Some(path.into()),
                        error: Some(error.into()),
                        excerpt,
                    };
                    let failed = failed_record(session, &unit, &failure);
                    let stored = Stored::read(&failed.rationale).unwrap();
                    let resolved = resolved_record(session, &unit, &failed, stored.rationale);
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
        let epoch = memo.pin("s1");
        assert!(memo.busy("s1"), "one checkpoint of a session at a time");
        assert!(!memo.busy("s2"));
        for call in 0..MEMO_CALLS_PER_SESSION as u64 + 1 {
            memo.learn("s1", epoch, call);
        }
        assert_eq!(memo.calls_for_test("s1"), MEMO_CALLS_PER_SESSION);
        assert!(!memo.has_seen("s1", 0), "the call learned first gives way");
        assert!(memo.has_seen("s1", 1));
        // A session's delete ends the entry: its checkpoint learns nothing
        // more, and the session admits its next checkpoint once the running
        // one has ended.
        memo.forget_session("s1");
        assert!(!memo.current("s1", epoch));
        assert!(
            memo.busy("s1"),
            "the deleted session's checkpoint still runs"
        );
        memo.learn("s1", epoch, 7);
        assert_eq!(memo.calls_for_test("s1"), 0);
        memo.release("s1", epoch);
        assert!(!memo.busy("s1"));
        let later = memo.pin("s1");
        memo.release("s1", epoch);
        assert!(
            memo.busy("s1") && memo.current("s1", later),
            "a stale release"
        );
        memo.release("s1", later);
        assert_eq!(memo.active_for_test(), 0);
        // Checkpoints of other sessions run up to their bound.
        let held: Vec<(String, u64)> = (0..MAX_CHECKPOINTS)
            .map(|index| {
                let session = format!("held-{index}");
                let epoch = memo.pin(&session);
                (session, epoch)
            })
            .collect();
        assert!(memo.busy("s1"), "the bound of checkpoints runs");
        let (session, epoch) = &held[0];
        memo.release(session, *epoch);
        assert!(!memo.busy("s1"));
        // A full memo gives up its oldest entry whose session runs no
        // checkpoint.
        for index in 0..MEMO_SESSIONS {
            let session = format!("filler-{index}");
            let epoch = memo.pin(&session);
            memo.release(&session, epoch);
        }
        assert_eq!(memo.sessions.len(), MEMO_SESSIONS);
        assert!(
            memo.sessions.contains_key("held-1"),
            "a running entry stays"
        );
        assert!(!memo.sessions.contains_key("s1"));
    }
}

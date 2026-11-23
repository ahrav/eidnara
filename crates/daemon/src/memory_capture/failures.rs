//! Records of failed commands, written from tool results without a model
//! call.
//!
//! A later session is asked what went wrong the last time a command failed.
//! Capture reads message text, and the error a command printed stands in its
//! tool result. Each transform pass carries the window's tool calls and
//! results, so the daemon reads them there: a result of a tool call that
//! carries a command line becomes one labeled memory row of category
//! [`FAILURE_CATEGORY`] when the harness marks the result as an error. The
//! mark alone decides: Pi sets it on the result of a command that exits
//! with a nonzero status, and an unmarked result reports no failure. The row
//! is served on explicit search and reaches no automatic surface: the block
//! renders positive categories only.
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
//! One row is live per command line of a project. The rows of one command
//! line share a source lineage, `tool_failure:<digest>`, and the Kernel's
//! newest row on that lineage is the command's record: a later marked result
//! supersedes it, and a later unmarked result rewrites it as resolved.
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
use super::{CAPTURE_SOURCE_KIND, COMMIT_WAIT, GATE_POLL, capture_enabled};
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
/// A tool call's id and a tool's name that are read, in bytes each. A
/// harness writes ids and names far below this, and a longer one is left
/// unread before a byte of it is copied.
const MAX_NAME_BYTES: usize = 1024;
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
/// one of them. Naming the command holds the redactor's copies of the
/// command line. Reading the output holds the table of its lines, one entry
/// of 32 bytes for each line and so at most 32 bytes for each byte of the
/// output and one entry more. Judging the output holds the redactor's
/// copies of it. Eight copies of a text of [`MAX_OUTPUT_BYTES`] cover the
/// copies of each of these phases, which follow one another.
const CHECKPOINT_SCRATCH_BYTES: usize = (32 + 8) * MAX_OUTPUT_BYTES;
/// Marks a line or a word cut to its byte limit.
const CUT_MARK: &str = "…";
/// The words Pi's shell tool writes before the exit code, in the last line
/// of the result of a command that exited with a nonzero status.
const EXIT_LINE: &str = "Command exited with code ";
/// The digits of an exit code that are read: a 32-bit status has ten.
const MAX_EXIT_CODE_DIGITS: usize = 10;
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

/// One line chosen from a failed result, within [`MAX_LINE_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Kept {
    text: String,
    /// The line is one of the output's closing lines.
    closing: bool,
}

/// What a marked result states, before the redactor has judged its output
/// (see [`reported_of`], [`screened`]).
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    /// The exit code the harness states in the last non-empty line (see
    /// [`harness_exit_code`]), with the bytes of that line.
    stated: Option<(i64, Range<usize>)>,
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

/// A marked result as a record states it. Every text here has passed the
/// redactor.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Failure {
    /// The nonzero exit code the harness states.
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
    /// The harness left the result unmarked.
    Passed,
    /// The harness marked the result as an error.
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
    /// The text of a result the harness marked. The text of an unmarked
    /// result decides nothing and stays in the window.
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
    /// The digest that names the lineage of the command's records (see
    /// [`command_named`]).
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
/// pass the reading of results an earlier pass judged. The Kernel's commit
/// log holds the receipt of each commit, so a call that wrote a row writes
/// nothing when it is judged again after a restart or after the memo gave
/// it up. A call that wrote no row left no receipt and is judged again as
/// a new result, by the same rules and against the records as they then
/// stand.
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
    /// Runs under the capture publication gate ahead of each judgment, so a
    /// test orders a judgment against another holder of the gate.
    #[cfg(test)]
    pub(crate) under_gate: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Holds the capture publication gate for a `session.delete` and retires
/// the session's memo entry when it is raised and again when it is dropped.
/// The drop retires the entry and releases the gate while it holds the
/// memo, so a checkpoint of the session is pinned either before that
/// retirement, and then finds its entry gone when it takes the gate, or
/// after the gate's release, as a pass handed off after the delete.
pub(crate) struct DeleteFence<'a> {
    gate: Option<std::sync::MutexGuard<'a, ()>>,
    memo: &'a Mutex<FailureMemo>,
    session: &'a str,
}

impl<'a> DeleteFence<'a> {
    pub(crate) fn raise(
        gate: std::sync::MutexGuard<'a, ()>,
        memo: &'a Mutex<FailureMemo>,
        session: &'a str,
    ) -> Self {
        // A poisoned memo is left to the lock that reports it.
        if let Ok(mut memo) = memo.lock() {
            memo.forget_session(session);
        }
        Self {
            gate: Some(gate),
            memo,
            session,
        }
    }
}

impl Drop for DeleteFence<'_> {
    fn drop(&mut self) {
        let memo = self.memo.lock();
        if let Ok(mut memo) = memo {
            memo.forget_session(self.session);
            // The gate opens before the memo does.
            drop(self.gate.take());
        }
    }
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
    /// next request, where the Kernel's receipts stop a second write of a
    /// call that wrote a row.
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

    fn forget_session(&mut self, session: &str) {
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

    /// The id of the record a call writes and the lineage of its command's
    /// records, for a test that places another row there.
    #[cfg(test)]
    pub(crate) fn ids_for_test(
        scope: &str,
        session: &str,
        call: &str,
        command: &str,
    ) -> (String, String) {
        let digest = command_named(scope, command).unwrap().digest;
        (
            record_id(scope, session, call),
            format!("{FAILURE_SOURCE_ID}:{digest}"),
        )
    }
}

/// The field that carries a tool call's command line: `command` when it is
/// a string with a character other than whitespace, else `cmd` when it is
/// one. A shell tool of any name carries one. An array of strings is the
/// arguments of one program and no command line, so a call that carries
/// one alone carries no command line.
fn command_field(input: &Value) -> Option<&str> {
    ["command", "cmd"]
        .into_iter()
        .filter_map(|name| input.get(name)?.as_str())
        .find(|text| !text.trim().is_empty())
}

/// The command line a tool call carries, when it is at most
/// [`MAX_OUTPUT_BYTES`]. The length is read before a byte is copied.
fn command_of(input: &Value) -> Option<String> {
    command_field(input)
        .filter(|text| text.len() <= MAX_OUTPUT_BYTES)
        .map(str::to_owned)
}

/// The text of a tool result and whether the harness marked it as an error:
/// the text of a `text` or `error_text` result, and the one text block of a
/// `content` or `error_content` result, beside which blocks of other kinds
/// may stand. `None` for a result that is no text of one unit: a `json` or
/// `error_json` value, which Pi's codec never gives, a result the harness
/// denied, and a `content` or `error_content` result with no text block or
/// with several, since the redactor judges each block as a unit of its own.
/// `None` also for an empty text, which is what a Pi result without
/// readable content decodes to (Pi's shell tool writes `(no output)` for a
/// command that printed nothing), and for a text longer than
/// [`MAX_OUTPUT_BYTES`], which source capture retains no span of.
fn text_of(output: &OutputKind) -> Option<(&str, bool)> {
    fn single(blocks: &[memory_store::ResultBlock]) -> Option<&String> {
        let mut texts = blocks.iter().filter_map(|block| match &block.kind {
            ResultBlockKind::Text { text } => Some(text),
            _ => None,
        });
        let text = texts.next()?;
        texts.next().is_none().then_some(text)
    }
    fn whole(text: &str) -> Option<&str> {
        (text.len() <= MAX_OUTPUT_BYTES && !text.is_empty()).then_some(text)
    }
    match output {
        OutputKind::Text { text } => Some((whole(text)?, false)),
        OutputKind::ErrorText { text } => Some((whole(text)?, true)),
        OutputKind::Content { blocks } => Some((whole(single(blocks)?)?, false)),
        OutputKind::ErrorContent { blocks } => Some((whole(single(blocks)?)?, true)),
        OutputKind::Json { .. }
        | OutputKind::ErrorJson { .. }
        | OutputKind::ExecutionDenied { .. } => None,
    }
}

/// Whether `word` is a `NAME=value` assignment as a shell reads one bare:
/// a name of ASCII letters, digits, and `_` that begins with no digit, `=`,
/// and a value that holds no quote, backslash, `$`, backtick, `<`, or `>`.
/// A value with one of those can hold whitespace, an expansion, a
/// redirection, or a mark of the redactor, and its word stays a word of the
/// command.
fn assigns(word: &str) -> bool {
    let name = word
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        .count();
    name > 0
        && !word.as_bytes()[0].is_ascii_digit()
        && word[name..].starts_with('=')
        && !word.contains(['\'', '"', '\\', '$', '`', '<', '>'])
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

/// What a record shows of a redacted command line: its program and the
/// program's first plain operand. The line is read as the words that
/// whitespace parts, after each leading `cd <directory> &&` step whose
/// directory is a plain word that begins with no `-` and after the leading
/// `NAME=value` words (see [`assigns`]) that a word follows. The first of
/// the remaining words is shown when it is a plain word, the word after it
/// when that is a plain word that begins with no `-`, and
/// [`ARGUMENTS_NOTE`] when more words follow; [`LEFT_OUT`] stands for a
/// line whose first such word is no plain word. Quotes and operators are
/// read as characters of their words, so a quoted or joined word is no plain
/// word.
fn shown_of(line: &str) -> String {
    let mut words = line.split_whitespace().peekable();
    loop {
        let mut rest = words.clone();
        let moves = rest.next() == Some("cd")
            && rest
                .next()
                .is_some_and(|directory| plain_word(directory) && !directory.starts_with('-'))
            && rest.next() == Some("&&")
            && rest.peek().is_some();
        if !moves {
            break;
        }
        words = rest;
    }
    let mut rest = words.clone();
    while rest.next_if(|word| assigns(word)).is_some() {}
    if rest.peek().is_some() {
        words = rest;
    }
    let Some(program) = words.next().filter(|word| plain_word(word)) else {
        return LEFT_OUT.to_owned();
    };
    let mut shown = program.to_owned();
    if let Some(operand) = words.next_if(|word| plain_word(word) && !word.starts_with('-')) {
        shown.push(' ');
        shown.push_str(operand);
    }
    if words.next().is_some() {
        shown.push_str(ARGUMENTS_NOTE);
    }
    shown
}

/// The command line as a record names it, or `None` when the redactor's
/// scan of it does not finish. The command line passes the redactor first,
/// so the words shown and the digest derive from redacted text: two command
/// lines that differ in a credential the redactor finds are one command.
///
/// The digest names the command line as it is written: the SHA-256 of the
/// project and the whole redacted line, each behind its length, and then,
/// for each finding of the redactor in order, the count of the command
/// line's bytes between the finding before it and that finding. Two
/// spellings of one operation, such as a run with and without a leading
/// `cd` or with other spacing, are two commands with a record each, so a
/// run of one spelling changes the record of that spelling alone. The
/// counts hold no byte and no length of a finding, so a credential of
/// another length keeps the digest, and a command line that spells the
/// redactor's mark itself has another digest than one in which the
/// redactor wrote that mark.
///
/// The words shown (see [`shown_of`]) are a reading aid: the digest alone
/// names the command, and every other argument stays out of the record.
fn command_named(scope: &str, command: &str) -> Option<Command> {
    let scanned = context_core::redaction::redact_windowed_durable_text(command, usize::MAX);
    let scanned = scanned.ok()?;
    let line = scanned.text;
    let mut hash = Sha256::new();
    for part in [scope, line.as_str()] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    // The bytes between two findings place each of the redactor's marks,
    // so a mark the command line itself spells is no finding's mark.
    let mut findings: Vec<(usize, usize)> = scanned
        .detections
        .iter()
        .map(|finding| (finding.offset, finding.offset + finding.length))
        .collect();
    findings.sort_unstable();
    let mut end = 0;
    for (start, next) in findings {
        hash.update((start.saturating_sub(end) as u64).to_le_bytes());
        end = end.max(next);
    }
    let digest = format!("{:x}", hash.finalize())[..32].to_owned();
    Some(Command {
        shown: shown_of(&line),
        digest,
    })
}

/// The exit code the harness states in `line`: [`EXIT_LINE`] and then one
/// to [`MAX_EXIT_CODE_DIGITS`] ASCII digits, which is the whole last line
/// Pi's shell tool writes for a command that exited with a nonzero status.
fn harness_exit_code(line: &str) -> Option<i64> {
    let digits = line.strip_prefix(EXIT_LINE)?;
    let whole = (1..=MAX_EXIT_CODE_DIGITS).contains(&digits.len())
        && digits.bytes().all(|byte| byte.is_ascii_digit());
    whole.then(|| digits.parse().ok()).flatten()
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
/// `Exception`, or the shell's [`MISSING_COMMAND`] words. The rule chooses
/// which lines of a marked result to keep; it decides no failure.
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

/// What the text of a marked result states, before the redactor has judged
/// it: the exit code of the harness's status line and the chosen lines.
///
/// The harness's error mark is the failure. The exit code is read from the
/// last non-empty line alone, which is where Pi's shell tool states it (see
/// [`harness_exit_code`]); a marked result that ends in another line states
/// no code.
///
/// The chosen lines are the first lines that state an error, each failure
/// line with the line that completes it, and then the closing lines of the
/// output. A line with a control character or a Unicode line separator is
/// left out, and a text that several lines share is chosen once.
fn reported_of(text: &str) -> Reported {
    let mut start = 0;
    // Each line, trimmed, with the bytes it stands at.
    let mut lines: Vec<(&str, Range<usize>)> = Vec::with_capacity(text.split('\n').count());
    for line in text.split('\n') {
        let span = start..start + line.len();
        start = span.end + 1;
        lines.push((line.trim(), span));
    }
    let shown = |line: &str| {
        !line.is_empty() && printable(line) && !line.contains(['\u{2028}', '\u{2029}'])
    };
    let stated = lines
        .iter()
        .rev()
        .find(|(line, _)| !line.is_empty())
        .and_then(|(line, span)| Some((harness_exit_code(line)?, span.clone())));
    let mut errors: Vec<usize> = Vec::new();
    let mut chosen: HashSet<String> = HashSet::new();
    for (index, (line, _)) in lines.iter().enumerate().filter(|(_, line)| shown(line.0)) {
        let failure = is_output_failure_line(line);
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
    Reported {
        stated,
        lines: kept,
    }
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

/// A marked result as a record states it, once the redactor has judged the
/// output `text` that `reported` was read from.
///
/// The record keeps the exit code when it is nonzero and the redactor finds
/// nothing in its line.
///
/// The text kept follows source capture's rule for a tool span: text in
/// which the redactor finds nothing is kept as it is, and text with a
/// finding is withheld whole. With no finding the chosen lines stand in
/// the record. With a finding the record holds no line, and the path and
/// the error name come from lines that hold no finding; when the scan does
/// not finish, no text of the output reaches the record.
fn screened(reported: Reported, text: &str) -> Failure {
    let Reported { stated, lines } = reported;
    let findings = findings_in(text);
    let clear = |span: &Range<usize>| {
        findings.as_ref().is_some_and(|findings| {
            findings
                .iter()
                .all(|finding| finding.end <= span.start || span.end <= finding.start)
        })
    };
    let exit_code = stated
        .filter(|(code, span)| *code != 0 && clear(span))
        .map(|(code, _)| code);
    let (path, error) = named_in(&lines, text, clear);
    let excerpt = match findings.as_deref() {
        Some([]) => Excerpt::Kept(lines.into_iter().map(|(line, _)| line).collect()),
        Some(_) => Excerpt::Finding,
        None => Excerpt::Unscanned,
    };
    Failure {
        exit_code,
        path,
        error,
        excerpt,
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
    let stated = match failure.exit_code {
        Some(code) => format!("the command exited with code {code}"),
        None => "the harness marked the result as an error".to_owned(),
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
    /// under that consent. The consent read is the one the session's route
    /// saved when it was bound. A result is paired with the call it
    /// answers in the tool turn under way: a command call of the assistant
    /// message before it, with no message of another role between them. A
    /// result under an execution id that two tool calls or two tool results
    /// of the window carry is left out, since the id names no one
    /// execution. The pass considers the last [`MEMO_CALLS_PER_SESSION`]
    /// command results of the window and hands off the earliest
    /// [`MAX_RESULTS_PER_PASS`] unjudged ones whose commands and outputs
    /// fit [`MAX_HANDOFF_BYTES`] together; each is copied within
    /// [`MAX_OUTPUT_BYTES`] for its command and for its text and within
    /// [`MAX_NAME_BYTES`] for its id and for its tool's name, so every
    /// single result fits, and a result that [`Input::of`] leaves out is
    /// handed off by name alone and settled unread. The task holds no
    /// message of the pass. One checkpoint of a session runs
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
        if !consents || !capture_enabled(binding) || self.kernel.kernel_store().is_err() {
            return None;
        }
        let scope = binding.kernel_project.digest();
        // One command result: its execution id, the call's tool and input,
        // the result's output, and the result's creation time.
        type Candidate<'a> = (&'a str, &'a str, &'a Value, &'a OutputKind, Option<i64>);
        let mut calls: HashMap<&str, (&str, &Value)> = HashMap::new();
        let (mut named, mut answered, mut shared) =
            (HashSet::new(), HashSet::new(), HashSet::new());
        let mut considered: VecDeque<Candidate<'_>> = VecDeque::new();
        for message in &request.messages {
            // A result answers a call of the tool turn under way: the calls
            // of the assistant message before it, up to the next message
            // that is neither the assistant's nor a tool's.
            let role = message.ck.role.as_str();
            if role == "assistant" {
                calls.clear();
            }
            for block in message.ck.content() {
                match block.kind() {
                    BlockKind::ToolCall {
                        id, name, input, ..
                    } => {
                        let id = execution_id(block, id);
                        if !named.insert(id) {
                            shared.insert(id);
                        }
                        if role == "assistant" && command_field(input).is_some() {
                            calls.insert(id, (name.as_str(), input));
                        }
                    }
                    BlockKind::ToolResult { id, output, .. } => {
                        let id = execution_id(block, id);
                        if !answered.insert(id) {
                            shared.insert(id);
                        }
                        let Some((tool, input)) = calls.remove(id) else {
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
            if role != "assistant" && role != "tool" {
                calls.clear();
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
    /// [`command_of`]), one whose call id or tool name passes
    /// [`MAX_NAME_BYTES`], and one whose message carries no creation time
    /// or a time outside the years the calendar renders, which has no place
    /// among the command's runs. Each length is read before a byte is
    /// copied.
    fn of(
        id: &str,
        tool: &str,
        input: &Value,
        output: &OutputKind,
        at_ms: Option<i64>,
    ) -> Option<Self> {
        let at_ms = at_ms.filter(|at_ms| utc_time(*at_ms).is_some())?;
        if id.len() > MAX_NAME_BYTES || tool.len() > MAX_NAME_BYTES {
            return None;
        }
        let command = command_of(input)?;
        let (text, flagged) = text_of(output)?;
        Some(Self {
            call: id.to_owned(),
            tool: tool.to_owned(),
            command,
            text: if flagged {
                text.to_owned()
            } else {
                String::new()
            },
            flagged,
            at_ms,
        })
    }

    /// The bytes this copy holds.
    fn bytes(&self) -> usize {
        self.call.len() + self.tool.len() + self.command.len() + self.text.len()
    }

    /// Reads the command and the result's outcome: a failure for a result
    /// the harness marked, and a pass for any other. `Err` for a result
    /// that changes no record, with the reason an operator can act on: a
    /// tool name or a call id that the redactor would change or that holds
    /// a control character or a Unicode line separator, and a command line
    /// whose redactor scan does not finish. The tool's name and the call's
    /// id are judged whole, before they are cut for display.
    fn observe(&self, scope: &str) -> Result<Observed, &'static str> {
        if !(names_whole(&self.tool) && names_whole(&self.call)) {
            return Err(
                "the redactor changes the tool's name or the call's id, or one of them holds a control character",
            );
        }
        let command = command_named(scope, &self.command)
            .ok_or("the redactor's scan of the command line did not finish")?;
        let outcome = match self.flagged {
            true => Outcome::Failed(screened(reported_of(&self.text), &self.text)),
            false => Outcome::Passed,
        };
        Ok(Observed {
            call: self.call.clone(),
            tool: tool_shown(&self.tool),
            command,
            at_ms: self.at_ms,
            outcome,
        })
    }
}

/// Whether a tool's name, a call's id, or a session's id stands in a record
/// as it is: the redactor passes it whole, and it holds no control
/// character and no Unicode line separator, so it opens no line of its own
/// in the record's text.
fn names_whole(text: &str) -> bool {
    kernel::exact_text_refusal(text).is_none()
        && !text
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
}

/// Whether a Kernel error leaves a call for a later pass: the Kernel is
/// busy or failing, or another writer changed a row under the judgment.
/// Any other refusal would meet the same result again.
const fn leaves_for_later(error: &kernel::KernelError) -> bool {
    use kernel::KernelError::{Busy, Deadline, Fault, FenceLost, Held, Io, NotFound};
    matches!(
        error,
        Deadline | Busy | Held | Io | FenceLost | Fault | NotFound
    )
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
    /// the task ends once they have. A daemon shutdown that came first ends
    /// the task before any of them; one that comes later raises the task's
    /// interrupt, which ends the pass before its next result, ends its wait
    /// for the capture publication gate, and ends a wait for the Kernel
    /// that is under way. A panic of the pass is reported through the
    /// daemon's redacting panic boundary. Returns the number of rows
    /// written.
    pub(crate) async fn run(self) -> usize {
        if self.cancel.is_cancelled() {
            return 0;
        }
        let interrupt = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let raised = Arc::clone(&interrupt);
        let mut join = tokio::task::spawn_blocking(move || {
            host_runtime::redact_panics(|| self.record(&raised))
        });
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
    /// memo entry is the one this checkpoint pinned and holds no judgment
    /// of the call; the memo learns a call once its judgment has ended. The
    /// gate is taken by polling, so a shutdown ends the wait for it. A
    /// Kernel that is busy or failing, and a row another writer changed
    /// under the judgment, end the pass with the call unlearned (see
    /// [`leaves_for_later`]), so a later pass whose window still carries
    /// the call and its result judges it and the results after it again, in
    /// the order they ran. Any other refusal of the Kernel settles the
    /// call. A panic under the gate leaves the gate usable: the guard is
    /// released before the panic goes on. A refusal is reported on stderr
    /// after the gate is released, by a fixed sentence that holds no text
    /// of the result.
    fn record(self, interrupt: &Arc<AtomicBool>) -> usize {
        let Self {
            kernel,
            commit_gate,
            project,
            pin,
            pending,
            cancel,
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
        let left = |reason: &dyn std::fmt::Display| {
            eprintln!("daemon: tool failure record left unwritten: {reason}");
        };
        // The session's id stands in a record's text and rationale, so it is
        // judged whole, once.
        let session_whole = names_whole(&pin.session);
        if !session_whole {
            left(&"the redactor changes the session's id, or it holds a control character");
        }
        let stopped = || cancel.is_cancelled() || interrupt.load(Ordering::Acquire);
        let mut written = 0;
        for result in &pending {
            if stopped() {
                break;
            }
            let unit = match &result.input {
                Some(input) if session_whole => input.observe(writer.scope).map_err(Some),
                _ => Err(None),
            };
            // The gate orders the Kernel reads, the commit, and the memo
            // update of every checkpoint, and session deletion takes it too.
            let gate = loop {
                if stopped() {
                    return written;
                }
                match commit_gate.try_lock() {
                    Ok(gate) => break gate,
                    Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(GATE_POLL),
                    Err(std::sync::TryLockError::Poisoned(_)) => {
                        panic!("capture publication mutex")
                    }
                }
            };
            let judged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let memo = || pin.memo.lock().expect("failure memo mutex");
                if stopped() || !memo().current(&pin.session, pin.epoch) {
                    return None;
                }
                if memo().has_seen(&pin.session, result.digest) {
                    return Some(Ok(Judged::Nothing));
                }
                #[cfg(test)]
                {
                    let hook = memo().under_gate.clone();
                    if let Some(hook) = hook {
                        hook();
                    }
                }
                let budget =
                    EvalBudget::new(Some(Instant::now() + COMMIT_WAIT), Arc::clone(interrupt));
                let judged = match &unit {
                    Ok(unit) => writer.judge(unit, &budget),
                    Err(Some(reason)) => Ok(Judged::Left(reason)),
                    Err(None) => Ok(Judged::Nothing),
                };
                if !matches!(&judged, Err(error) if leaves_for_later(error)) {
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
                Ok(Some(Ok(Judged::Left(reason)))) => left(&reason),
                Ok(Some(Err(error))) => {
                    left(&error);
                    if leaves_for_later(&error) {
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

/// Whether `id` has the form of a record's id: `mem_` and 32 lowercase
/// hexadecimal digits, as [`record_id`] writes one. Every key that ends in
/// such an id has one length.
fn is_record_id(id: &str) -> bool {
    id.strip_prefix("mem_").is_some_and(|digits| {
        digits.len() == 32
            && digits
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
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
    /// The session whose commit wrote the record: the one that ran the
    /// failed command, or the one that ran the passing command once the
    /// record is resolved.
    session: String,
}

impl Stored {
    /// `None` for a rationale that states no run as the recorder writes
    /// one: a state of `failed` or `resolved`, the session of that run, and
    /// its time, which the calendar renders.
    fn read(rationale: &str) -> Option<Self> {
        let Ok(Value::Object(rationale)) = serde_json::from_str(rationale) else {
            return None;
        };
        let (resolved, time, session) = match rationale.get("state").and_then(Value::as_str)? {
            "failed" => (false, "at_ms", "session"),
            "resolved" => (true, "resolved_at_ms", "resolved_session"),
            _ => return None,
        };
        let at_ms = rationale
            .get(time)
            .and_then(Value::as_i64)
            .filter(|at_ms| utc_time(*at_ms).is_some())?;
        let session = rationale.get(session).and_then(Value::as_str)?.to_owned();
        Some(Self {
            rationale,
            resolved,
            at_ms,
            session,
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
    /// The operation key of the commit that wrote, or writes, the record
    /// `id` for a run of `session`.
    fn record_key(&self, session: &str, id: &str) -> Result<String, kernel::KernelError> {
        let (session_key, _) = session_keys(session);
        self.project
            .operation_key(FAILURE_SOURCE_ID, &format!("{session_key}{id}"))
            .ok_or(kernel::KernelError::InvalidInput)
    }

    /// Whether the Kernel's commit log holds a commit of this recorder
    /// under `key`, which ends in an id of a record's form (see
    /// [`is_record_id`]). Such ids have one length, so the keys from this
    /// one to its successor in byte order are this key alone.
    fn committed(&self, key: &str, budget: &EvalBudget) -> Result<bool, kernel::KernelError> {
        let held = self.kernel.commit_count_in_key_range_within_budget(
            budget,
            FAILURE_SOURCE_ID,
            (key, &format!("{key}~")),
            1,
        )?;
        Ok(held > 0)
    }

    /// Writes what one command result calls for. The caller holds the
    /// capture publication gate, so no other checkpoint writes between the
    /// reads and the commit. Every wait for a Kernel connection ends at the
    /// budget's deadline, the reads of the log and of the lineage stop
    /// there between the steps of their statements, and the read of the
    /// newest decision's text bounds its wait for a connection alone.
    ///
    /// A call whose commit the Kernel's log holds writes nothing: the log's
    /// receipt under this recorder's producer name is the proof that the
    /// call's record was written, whatever became of the row. The command's
    /// record is the newest decision of its lineage, live or retired, when
    /// this recorder wrote it: the decision is a failure record of this
    /// project, its rationale states its run (see [`Stored::read`]), and
    /// the commit log holds the recorder's commit of that row for that
    /// run's session. A lineage whose newest decision is anything else is
    /// left as it is. A run follows a record when its result's creation
    /// time is greater than the time of the record's run. A failure writes
    /// the first row of a lineage, a row after a retired record that it
    /// follows, and a row in place of a live record that it follows. A pass
    /// rewrites a live, unresolved record that it follows as resolved.
    /// Either writes while the session has written fewer than
    /// [`MAX_RECORDS_PER_SESSION`] records by the Kernel's commit log.
    fn judge(&self, unit: &Observed, budget: &EvalBudget) -> Result<Judged, kernel::KernelError> {
        let id = record_id(self.scope, self.session, &unit.call);
        let key = self.record_key(self.session, &id)?;
        if self.committed(&key, budget)? {
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
                if row.superseded_by.is_some() || !is_record_id(&row.object_id) {
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
                    .pop();
                let stored = decision.and_then(|decision| {
                    let owned = decision.decision_kind == FAILURE_CATEGORY
                        && decision.scope_id.as_deref() == Some(self.project.scope_id().as_str());
                    let stored = Stored::read(&decision.payload.rationale).filter(|_| owned)?;
                    Some((decision.payload, stored))
                });
                let Some((payload, stored)) = stored else {
                    return Ok(unread);
                };
                let wrote = self.record_key(&stored.session, &row.object_id)?;
                if !self.committed(&wrote, budget)? {
                    return Ok(unread);
                }
                if unit.at_ms <= stored.at_ms {
                    return Ok(Judged::Nothing);
                }
                (revision, Some((row, payload, stored)))
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

    /// The exit code and the kept lines of a marked text result.
    fn failed(text: &str) -> (Option<i64>, Vec<String>) {
        let failure = screened(reported_of(text), text);
        let lines = match failure.excerpt {
            Excerpt::Kept(lines) => lines.into_iter().map(|line| line.text).collect(),
            Excerpt::Finding | Excerpt::Unscanned => Vec::new(),
        };
        (failure.exit_code, lines)
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

    /// What a text result of `command` says, as the task reads it.
    fn outcome(command: &str, text: &str, flagged: bool) -> Outcome {
        input(command, text, flagged)
            .observe(SCOPE)
            .unwrap()
            .outcome
    }

    /// The failure a text result of `command` records: `None` for a pass.
    fn recorded(command: &str, text: &str, flagged: bool) -> Option<Failure> {
        match outcome(command, text, flagged) {
            Outcome::Passed => None,
            Outcome::Failed(failure) => Some(failure),
        }
    }

    #[test]
    fn a_command_line_is_read_from_the_call_within_its_bytes() {
        assert_eq!(
            command_of(&json!({ "command": "cargo test" })).as_deref(),
            Some("cargo test")
        );
        assert_eq!(
            command_of(&json!({ "command": "  ", "cmd": "npm run build" })).as_deref(),
            Some("npm run build")
        );
        // An array of strings is the arguments of one program and no
        // command line.
        assert_eq!(
            command_of(&json!({ "command": ["npm", "run"], "cmd": "make" })).as_deref(),
            Some("make")
        );
        for input in [
            json!({ "path": "src/lib.rs" }),
            json!({ "command": "" }),
            json!({ "command": ["npm", "run", "build"] }),
            json!({ "cmd": ["sh", "-c", "make | tail"] }),
            json!({ "command": 7 }),
        ] {
            assert_eq!(command_of(&input), None, "{input}");
        }
        // A command line of the bound is read, and one byte more is left
        // out. A `command` past the bound falls back to no `cmd`.
        let at_bound = "x".repeat(MAX_OUTPUT_BYTES);
        assert!(command_of(&json!({ "command": at_bound })).is_some());
        let past = format!("{at_bound}x");
        assert_eq!(command_of(&json!({ "command": past, "cmd": "ls" })), None);
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
            assert_eq!(text_of(&output), Some((at_bound.as_str(), flagged)));
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
        // A result of blocks is the text of its one text block, beside
        // blocks of other kinds. One with no text block, such as an image
        // alone, and one with several text blocks is no text.
        let image = || memory_store::ResultBlock {
            kind: ResultBlockKind::Opaque {
                opaque: memory_store::OpaqueBlock {
                    source: json!("pi"),
                    kind: "image".into(),
                    raw: json!({ "type": "image" }),
                    arc: None,
                },
            },
            provider_extras: Default::default(),
        };
        let words = |text: &str| memory_store::ResultBlock {
            kind: ResultBlockKind::Text { text: text.into() },
            provider_extras: Default::default(),
        };
        assert_eq!(
            text_of(&OutputKind::ErrorContent {
                blocks: vec![image(), words("one\ntwo"), image()],
            }),
            Some(("one\ntwo", true))
        );
        assert_eq!(
            text_of(&OutputKind::Content {
                blocks: vec![words("(no output)")],
            }),
            Some(("(no output)", false))
        );
        for blocks in [
            Vec::new(),
            vec![image()],
            vec![words("one"), words("two")],
            vec![words("one"), image(), words("")],
            vec![words("")],
        ] {
            assert_eq!(
                text_of(&OutputKind::Content {
                    blocks: blocks.clone()
                }),
                None
            );
            assert_eq!(text_of(&OutputKind::ErrorContent { blocks }), None);
        }
        // An empty text is what a result without readable content decodes
        // to, and is no text either. A text of whitespace is a command's
        // own output.
        assert_eq!(
            text_of(&OutputKind::Text {
                text: String::new()
            }),
            None
        );
        assert_eq!(
            text_of(&OutputKind::ErrorText {
                text: String::new()
            }),
            None
        );
        assert_eq!(
            text_of(&OutputKind::Text { text: "\n".into() }),
            Some(("\n", false))
        );
        assert_eq!(
            text_of(&OutputKind::Content {
                blocks: vec![words(" \n")]
            }),
            Some((" \n", false))
        );
        assert_eq!(outcome("make check", "\n", false), Outcome::Passed);
        // A text that is one JSON object is text: its exit field decides
        // nothing, since a passing command can print one.
        let object = json!({ "exitCode": 137, "reason": "OOMKilled" }).to_string();
        assert_eq!(
            outcome("kubectl get pod api -o json", &object, false),
            Outcome::Passed
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
            ("make | tail", "make [arguments left out]"),
            ("make && ls", "make [arguments left out]"),
            // A word that an operator or a quote is written against is no
            // plain word.
            ("make; ls", LEFT_OUT),
            ("\"my tool\" run", LEFT_OUT),
            ("(cd api && make)", LEFT_OUT),
            ("$TOOL run", LEFT_OUT),
            // The words shown leave out each leading directory step and the
            // leading assignments.
            (
                "cd services && cargo test -p api",
                "cargo test [arguments left out]",
            ),
            (
                "cd services && cd api && RUST_LOG=debug cargo test",
                "cargo test",
            ),
            (
                "RUST_LOG=debug TZ=UTC cargo test -p api",
                "cargo test [arguments left out]",
            ),
            ("cd a && \\\n  cargo test", LEFT_OUT),
            ("cd services", "cd services"),
            ("cd services &&", "cd services [arguments left out]"),
            ("MODE=fast", LEFT_OUT),
            // A line break parts two words as a space does.
            (
                "python3 - <<'PY'\nprint(1)\nPY",
                "python3 [arguments left out]",
            ),
            // A directory step whose directory is an option or no plain
            // word, or that another operator joins, stays the command.
            ("cd -P && make check", "cd [arguments left out]"),
            ("cd 'my services' && make", "cd [arguments left out]"),
            ("cd services; make", "cd [arguments left out]"),
            ("cd $(a && b c) && make", "cd [arguments left out]"),
            // An assignment whose value is quoted, escaped, expanded, or
            // redirected stays a word of the command, so no word of a
            // quoted value is taken for the program.
            ("MSG=\"run make now\" ./deploy", LEFT_OUT),
            ("VERSION=$(git describe --tags) make release", LEFT_OUT),
            ("VERSION=`git describe --tags` make release", LEFT_OUT),
            ("NAME=two\\ words ./greet", LEFT_OUT),
            ("MODE=check>missing/trace.log true", LEFT_OUT),
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
    fn a_command_s_lineage_is_its_redacted_line_as_written() {
        let digest = |command: &str| named(command).digest;
        let plain = digest("cargo test -p api");
        assert_eq!(plain.len(), 32);
        assert_eq!(digest("cargo test -p api"), plain);
        // Every other spelling is another command, also where a shell runs
        // the same program: a leading directory step or assignment, the
        // width of a space, a line break for a `;`, a quoted word, a
        // comment, a carriage return, and whitespace at an end.
        for other in [
            "cargo test -p web",
            "cargo test",
            "cd services && cargo test -p api",
            "RUST_LOG=debug cargo test -p api",
            "cargo  test -p api",
            "cargo test -p 'api'",
            "cargo test -p api # in the workspace",
            "cargo test -p api\n",
            " cargo test -p api",
            "cargo test -p api\r",
            "cargo test -p api && cd ..",
        ] {
            assert_ne!(digest(other), plain, "{other:?}");
        }
        for (one, other) in [
            ("cargo test\nfalse", "cargo test;false"),
            ("make|tail", "make | tail"),
            ("make | tail", "make || tail"),
            ("cd \"$?\"; make; echo \"rc=$?\"", "make; echo \"rc=$?\""),
            ("cd -P && make check", "make check"),
            ("MODE=fast >/dev/null; make", "MODE=safe >/dev/null; make"),
            (
                "cat x | &>/dev/null grep -q .",
                "cat x |\\\n&>/dev/null grep -q .",
            ),
            ("A=${1name} true", "true"),
            ("cat <<EOF foo\nbar\nEOF", "cat <<EOF\nfoo bar\nEOF"),
        ] {
            assert_ne!(digest(one), digest(other), "{one:?} {other:?}");
        }
        assert_ne!(
            command_named("another project", "cargo test -p api")
                .unwrap()
                .digest,
            plain
        );
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
        for command in [
            deploy(&assignment),
            format!("deploy {assignment}"),
            format!("{assignment} deploy"),
        ] {
            let shown = named(&command).shown;
            assert!(!shown.contains(&value), "{shown}");
        }
        assert_eq!(
            digest(&format!("{assignment} make deploy")),
            digest(&format!("{rotated} make deploy"))
        );
        // A command line that spells the redactor's mark itself is another
        // command than one in which the redactor wrote that mark.
        let found = deploy(&assignment);
        let spelled =
            context_core::redaction::redact_windowed_durable_text(&found, usize::MAX).unwrap();
        assert_eq!(spelled.detections.len(), 1);
        let again =
            context_core::redaction::redact_windowed_durable_text(&spelled.text, usize::MAX)
                .unwrap();
        assert_eq!(
            (again.text.as_str(), again.detections.len()),
            (spelled.text.as_str(), 0),
            "the mark is text the redactor leaves as it is"
        );
        assert_ne!(digest(&found), digest(&spelled.text));
    }

    #[test]
    fn the_harness_states_an_exit_code_in_its_own_last_line() {
        for (line, code) in [
            ("Command exited with code 1", 1),
            ("Command exited with code 101", 101),
            ("Command exited with code 0", 0),
            ("Command exited with code 4294967295", 4_294_967_295),
        ] {
            assert_eq!(harness_exit_code(line), Some(code), "{line}");
        }
        for line in [
            "exit code 1",
            "Exit code: 2",
            "command exited with code 5",
            "Command exited with code 5.",
            "Command exited with code -1",
            "Command exited with code",
            "Command exited with code ",
            "Command exited with code 1 2",
            "Command exited with code 12345678901",
            "ELIFECYCLE Command failed with exit code 1.",
            "worker: Command exited with code 7",
        ] {
            assert_eq!(harness_exit_code(line), None, "{line}");
        }
    }

    #[test]
    fn the_harness_s_mark_alone_decides_a_failure() {
        let lines = |lines: &[&str]| -> Vec<String> {
            lines.iter().map(|line| (*line).to_owned()).collect()
        };
        // A marked result is a failure, and its code is read from the last
        // non-empty line alone.
        assert_eq!(
            failed("ok\n\nCommand exited with code 2\n"),
            (Some(2), lines(&["ok", "Command exited with code 2"]))
        );
        assert_eq!(
            failed("Command exited with code 3\nall done"),
            (None, lines(&["Command exited with code 3", "all done"]))
        );
        assert_eq!(
            failed("Command exited with code 0"),
            (None, lines(&["Command exited with code 0"]))
        );
        assert!(matches!(
            outcome("make", "ok\n\nCommand exited with code 2", true),
            Outcome::Failed(Failure {
                exit_code: Some(2),
                ..
            })
        ));
        // An unmarked result is a pass, whatever its text states: a status
        // line, the harness's own words, a runner's summary, a shell's
        // report, or the number a command line echoes.
        for (command, text) in [
            ("make", "error: boom\n\nCommand exited with code 1"),
            ("make", "Exit code: 4\nfirst\nsecond"),
            ("pnpm build", "ELIFECYCLE Command failed with exit code 1."),
            ("pnpm build", "bash: line 1: pnpm: command not found"),
            (
                "cargo test | tail -n 3",
                "test result: FAILED. 1 passed; 1 failed",
            ),
            ("make check; echo \"rc=$?\"", "rc=2"),
            ("make check || true", "make: *** [check] Error 1"),
        ] {
            assert_eq!(outcome(command, text, false), Outcome::Passed, "{text}");
        }
    }

    #[test]
    fn a_marked_result_the_harness_ended_itself_is_recorded_as_marked() {
        for text in [
            "partial output\n\nCommand aborted",
            "error: slow\n\nCommand timed out after 120 seconds",
            "Tool execution was blocked",
            "Validation failed for tool \"bash\":\n  - command: must be string",
        ] {
            let failure = recorded("make", text, true).unwrap();
            assert_eq!(failure.exit_code, None, "{text}");
            let Excerpt::Kept(kept) = failure.excerpt else {
                panic!("{text}");
            };
            let last = text.lines().next_back().unwrap().trim();
            assert_eq!(kept.last().map(|line| line.text.as_str()), Some(last));
            assert_eq!(outcome("make", text, false), Outcome::Passed, "{text}");
        }
    }

    #[test]
    fn pi_s_note_after_a_cut_output_is_a_line_of_the_result() {
        let note = "[Showing lines 3-9 of 9. Full output: /tmp/pi-bash-0123456789abcdef.log]";
        let text = format!(
            "error: boom at src/app.rs:3:1\nlast line\n\n{note}\n\nCommand exited with code 1"
        );
        let failure = recorded("make", &text, true).unwrap();
        assert_eq!(
            (failure.exit_code, failure.path.as_deref()),
            (Some(1), Some("src/app.rs:3:1"))
        );
        assert_eq!(
            failure.excerpt,
            Excerpt::Kept(
                [
                    ("error: boom at src/app.rs:3:1", false),
                    (note, true),
                    ("Command exited with code 1", true)
                ]
                .map(|(text, closing)| Kept {
                    text: text.to_owned(),
                    closing
                })
                .to_vec()
            )
        );
    }

    #[test]
    fn a_failed_result_keeps_its_first_error_lines_and_its_closing_lines() {
        let text = "Compiling api\nerror[E0308]: mismatched types\n  --> src/render.rs:9:5\nwarning: unused\nerror: aborting\nTraceback (most recent call last):\nValueError: bad\nfatal: stop\nERROR extra one\nERROR extra two\nlast but one\n\nCommand exited with code 1";
        let (_, kept) = failed(text);
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
        let (_, kept) = failed(&long);
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept[0].len() <= MAX_LINE_BYTES && kept[0].ends_with(CUT_MARK));
        assert_eq!(kept[1], "Command exited with code 1");
        // A text the output repeats is chosen once, so a repeated error
        // leaves its slots to the distinct errors after it.
        let repeated = format!(
            "{}error: the cause\nnote one\nnote two\nexit status 1\nexit status 1",
            "error: build failed\n".repeat(5)
        );
        let (code, kept) = failed(&repeated);
        assert_eq!(code, None, "a status line of the output's own");
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
                exit_code: None,
                path: None,
                error: None,
                excerpt: Excerpt::Unscanned,
            })
        );
        // The same text without the harness's mark is a pass, and its text
        // is left unread.
        assert_eq!(outcome("make", &block, false), Outcome::Passed);
    }

    #[test]
    fn an_exit_code_whose_line_holds_a_finding_reaches_no_record() {
        // The line before the harness's status line ends in an empty
        // assignment under a secret's name, and the redactor takes the next
        // word, the first of the status line, as its value.
        let key = ["api_", "tok", "en"].concat();
        let text = format!("export {key}=\nCommand exited with code 7");
        let status_line = text.len() - "Command exited with code 7".len()..text.len();
        let findings = findings_in(&text).unwrap();
        assert!(
            findings
                .iter()
                .any(|finding| finding.start < status_line.end && status_line.start < finding.end),
            "the fixture's finding stands in the status line: {findings:?}"
        );
        let marked = recorded("deploy", &text, true).unwrap();
        assert_eq!(
            (marked.exit_code, &marked.excerpt),
            (None, &Excerpt::Finding)
        );
        // The positive control: a status line without a finding keeps its
        // code beside a finding on another line.
        let clean = recorded(
            "deploy",
            &format!("{key}=fixture-v7\nCommand exited with code 3"),
            true,
        )
        .unwrap();
        assert_eq!(
            (clean.exit_code, clean.excerpt),
            (Some(3), Excerpt::Finding)
        );
    }

    #[test]
    fn a_tool_name_a_call_id_and_a_time_are_judged_whole_before_a_record_shows_them() {
        let (assignment, value) = keyed_credential();
        let failing = |edit: &dyn Fn(&mut Input)| {
            let mut input = input("make", "error: boom\nCommand exited with code 1", true);
            edit(&mut input);
            input.observe(SCOPE).map_err(|_| true)
        };
        assert!(failing(&|_| {}).is_ok(), "the positive control");
        // An id the redactor would change past the bytes a record shows
        // writes nothing: the id is judged before it is cut.
        let long_id = format!("{}{assignment}", "c".repeat(MAX_ID_BYTES));
        assert!(!clipped(&long_id, MAX_ID_BYTES).contains(&value));
        assert_eq!(
            failing(&|input| input.call = long_id.clone()).err(),
            Some(true)
        );
        let long_tool = format!("{} {assignment}", "t".repeat(MAX_TOOL_BYTES));
        assert!(!tool_shown(&long_tool).contains(&value));
        assert_eq!(
            failing(&|input| input.tool = long_tool.clone()).err(),
            Some(true)
        );
        // An id or a name with a control character or a Unicode line
        // separator would open a line of its own in the record's text.
        for broken in ["call\n1", "call\u{2028}1", "call\u{7}1", "call\t1"] {
            assert!(!names_whole(broken), "{broken:?}");
            assert_eq!(
                failing(&|input| input.call = broken.into()).err(),
                Some(true)
            );
            assert_eq!(
                failing(&|input| input.tool = broken.into()).err(),
                Some(true)
            );
        }
        for whole in ["toolu_01AbC-9", "call 1", "bash", "s1"] {
            assert!(names_whole(whole), "{whole}");
        }
        // The parts a record shows have the limits the wire document states.
        assert_eq!(
            (
                MAX_TOOL_BYTES,
                MAX_WORD_BYTES,
                MAX_ID_BYTES,
                MAX_TOKEN_BYTES
            ),
            (32, 64, 96, 96)
        );
        assert_eq!(tool_shown("  run   shell "), "run shell");
        assert_eq!(
            tool_shown(&"é".repeat(40)),
            format!("{}…", "é".repeat(14)),
            "32 bytes: 14 two-byte letters and the cut mark"
        );
        assert_eq!(
            tool_shown(&"word ".repeat(40)),
            "word word word word word word…",
            "whole words up to the limit, then the cut mark"
        );
        // A result without a creation time, or with one the calendar does
        // not render, is copied for no judgment.
        let call = json!({ "command": "make" });
        let output = OutputKind::ErrorText {
            text: "error: boom".into(),
        };
        let of = |at_ms| Input::of("call-1", "bash", &call, &output, at_ms);
        assert!(of(Some(1_000)).is_some());
        // An id or a tool name past 1,024 bytes is left out before a byte
        // of it is copied.
        let (at_bound, past) = ("i".repeat(1_024), "i".repeat(1_025));
        assert!(Input::of(&at_bound, &at_bound, &call, &output, Some(1_000)).is_some());
        assert!(Input::of(&past, "bash", &call, &output, Some(1_000)).is_none());
        assert!(Input::of("call-1", &past, &call, &output, Some(1_000)).is_none());
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

    fn failure(exit_code: Option<i64>, excerpt: Excerpt) -> Failure {
        Failure {
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
        for (code, stated) in [
            (Some(101), "the command exited with code 101"),
            (None, "the harness marked the result as an error"),
        ] {
            let failure = failure(code, Excerpt::Kept(lines.clone()));
            assert_eq!(
                summary_of(&unit, "s1", &failure),
                format!("{head}{stated}{tail}")
            );
        }
        // A command line that begins with no plain word is shown as left
        // out, in the text and in the rationale.
        let failure = failure(Some(1), Excerpt::Kept(lines));
        let quoted = observed("\"my tool\" run");
        let record = failed_record("s1", &quoted, &failure);
        assert!(
            record.summary.contains("\nCommand: [left out]\n"),
            "{}",
            record.summary
        );
        let rationale: Value = serde_json::from_str(&record.rationale).unwrap();
        assert_eq!(rationale["command"], LEFT_OUT);
        assert_eq!(rationale["exit_code"], 1);
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
            exit_code: Some(9_999_999_999),
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

    /// Each part of a record passes the redactor alone, and the record is
    /// judged again as composed: a tool name that ends in a secret's key
    /// and `=` takes the words after it in the first line as its value.
    #[test]
    fn a_name_that_passes_alone_is_judged_again_in_the_composed_record() {
        let tool = format!("run {}=", ["api_", "tok", "en"].concat());
        assert!(names_whole(&tool));
        let compose = |tool: &str| {
            let mut unit = observed("make");
            unit.tool = tool_shown(tool);
            let failure = failure(Some(1), Excerpt::Kept(Vec::new()));
            failed_record("s1", &unit, &failure)
        };
        let record = compose(&tool);
        assert!(kernel::exact_text_refusal(&record.summary).is_some());
        let plain = compose("bash");
        assert!(kernel::exact_text_refusal(&plain.summary).is_none());
        assert!(kernel::exact_text_refusal(&plain.rationale).is_none());
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
        // A record's id is `mem_` and 32 lowercase hexadecimal digits.
        let id = record_id("p", "s", "c");
        assert!(is_record_id(&id), "{id}");
        for other in [
            "mem_",
            &id[..35],
            &format!("{id}0"),
            &id.to_uppercase(),
            "mem_0123456789abcdef0123456789abcdEf",
            &id.replacen("mem_", "dec_", 1),
            "mem_0123456789abcdef0123456789abcdeg",
        ] {
            assert!(!is_record_id(other), "{other}");
        }
    }

    /// A delete's fence retires the session's entry when it is raised and
    /// again when it is dropped, so a checkpoint pinned before the delete
    /// and one pinned while it runs both find their entry gone. The
    /// checkpoint keeps its place among the running ones until it ends,
    /// and a pin after the fence is current.
    #[test]
    fn a_delete_fence_retires_the_session_s_entry_at_both_ends() {
        let memo = Mutex::new(FailureMemo::default());
        let gate = Mutex::new(());
        let pin = || memo.lock().unwrap().pin("s1");
        let current = |epoch| memo.lock().unwrap().current("s1", epoch);
        let before = pin();
        memo.lock().unwrap().learn("s1", before, 7);
        let fence = DeleteFence::raise(gate.lock().unwrap(), &memo, "s1");
        assert!(gate.try_lock().is_err(), "the fence holds the gate");
        assert!(!current(before), "the entry pinned before the delete");
        assert!(!memo.lock().unwrap().has_seen("s1", 7));
        let during = pin();
        assert!(current(during), "a pin while the fence stands is admitted");
        drop(fence);
        assert!(
            gate.try_lock().is_ok(),
            "the fence's drop releases the gate"
        );
        assert!(!current(during), "the entry pinned while the delete ran");
        assert!(
            memo.lock().unwrap().busy("s1"),
            "the fenced checkpoint keeps its place until it ends"
        );
        memo.lock().unwrap().release("s1", during);
        let after = pin();
        assert!(current(after));
        assert!(!memo.lock().unwrap().busy("other"));
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

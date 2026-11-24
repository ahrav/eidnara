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
//! with a finding is withheld whole. The record names its command by the
//! program's name and its first plain operand; its `Command:` line and its
//! rationale's `command` field hold no other argument of the command line.
//!
//! One row is live per command line of a project. The rows of one command
//! line share a source lineage, `tool_failure:<digest>`, and the Kernel's
//! newest row on that lineage is the command's record: a later marked result
//! supersedes it, and a later unmarked result rewrites it as resolved.
//!
//! A record's id derives from the session entry that holds the result and
//! from the tool call's id, so the daemon records the results of a harness
//! whose result rows carry an entry id of their own. Pi's do.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use kernel::applicability::EvalBudget;
use memory_store::{BlockKind, OutputKind, ResultBlockKind, WireBlock};
use regex::Regex;
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
/// the results of one command are judged in window order, the order the
/// harness wrote them.
const MAX_RESULTS_PER_PASS: usize = 256;
/// The bytes of commands and outputs one pass copies for its checkpoint.
const MAX_HANDOFF_BYTES: usize = 8 * 1024 * 1024;
/// Tool calls one session's memo remembers, and the command results at the
/// end of a window that a pass considers. The two are equal, so a call the
/// memo evicts has left the results a later pass considers while the
/// session's windows grow by appending.
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
/// one of them: the redactor's workspace while it names the command, and
/// then the chosen lines and the redactor's workspace while it judges the
/// output. The redactor holds copies of the text it scans and a list of
/// its findings with their replacements. The reserve is forty bytes for
/// each byte of the largest text read, [`MAX_OUTPUT_BYTES`]; the chosen
/// lines are seven of [`MAX_LINE_BYTES`], and the rest is the redactor's.
const CHECKPOINT_SCRATCH_BYTES: usize = 40 * MAX_OUTPUT_BYTES;

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
    /// The output begins with the heading of the harness's report of a
    /// refused call (see [`begins_refusal`]). The record withholds the
    /// output text.
    Refusal,
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

/// One command result of the window as a pass borrows it.
#[derive(Clone, Copy)]
struct Candidate<'a> {
    /// The id of the session entry that holds the result.
    entry: &'a str,
    /// The id of the tool call the result answers: Pi's native id.
    call: &'a str,
    tool: &'a str,
    /// The command line the call carries, as written.
    command: &'a str,
    output: &'a OutputKind,
    /// When the harness wrote the result, in milliseconds.
    at_ms: Option<i64>,
}

/// A command result that is read, as its fields stand in the window.
struct Readable<'a> {
    entry: &'a str,
    call: &'a str,
    tool: &'a str,
    command: &'a str,
    /// The text of a result the harness marked.
    marked: Option<&'a str>,
    at_ms: i64,
}

/// What the task reads of one command result, copied from the window within
/// [`MAX_OUTPUT_BYTES`] for the command and for the text, so the task holds
/// no message of the pass.
struct Input {
    /// The id of the session entry that holds the result. With the call's
    /// id it names the execution.
    entry: String,
    /// Pi's native tool call id.
    call: String,
    tool: String,
    command: String,
    /// The text of a result the harness marked. An unmarked result's text
    /// decides nothing and stays in the window.
    marked: Option<String>,
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
    entry: String,
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
    /// The Kernel history the judged calls were judged against. A restore
    /// installs another history, and the calls are then judged again.
    history: Option<kernel::CommitReadIncarnation>,
    /// The sessions whose checkpoint is admitted and has not finished, each
    /// with the epoch of the entry that checkpoint pinned. A session's
    /// delete removes its entry and leaves its place here, so the session
    /// admits its next checkpoint once the running one has ended.
    active: HashMap<String, u64>,
    /// Runs under the capture publication gate ahead of each judgment, so a
    /// test orders a judgment against another holder of the gate.
    #[cfg(test)]
    pub(crate) under_gate: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Runs under the gate between a judgment's Kernel reads and its commit.
    #[cfg(test)]
    pub(crate) before_commit: Option<Arc<dyn Fn() + Send + Sync>>,
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

/// The memo's name of one execution: the project, the session entry that
/// holds the result, and the call's id. The name lives in this process
/// alone.
fn call_digest(scope: &str, entry: &str, call: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (scope, entry, call).hash(&mut hasher);
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

    /// Remembers a call judged under the Kernel history `history`, and
    /// answers whether the memo holds it. The call is kept while `history`
    /// is the history the memo's calls stand in and the session's entry is
    /// the one `epoch` names; a memo that another pass has moved to another
    /// history, or to none, keeps its calls as they are. A full entry gives
    /// up the call it learned first, which has left the results a pass
    /// considers.
    fn learn(
        &mut self,
        session: &str,
        epoch: u64,
        call: u64,
        history: kernel::CommitReadIncarnation,
    ) -> bool {
        if self.history != Some(history) {
            return false;
        }
        let Some(entry) = self
            .sessions
            .get_mut(session)
            .filter(|entry| entry.epoch == epoch)
        else {
            return false;
        };
        if entry.calls.insert(call) {
            entry.order.push_back(call);
            while entry.order.len() > MEMO_CALLS_PER_SESSION {
                if let Some(evicted) = entry.order.pop_front() {
                    entry.calls.remove(&evicted);
                }
            }
        }
        true
    }

    fn forget_session(&mut self, session: &str) {
        if self.sessions.remove(session).is_some() {
            self.order.retain(|kept| kept != session);
        }
    }

    /// Takes `history` as the Kernel history the judged calls stand in.
    /// Under another history every session's judged calls are given up, so
    /// each is judged again against the history the Kernel now holds; the
    /// entries and their epochs stay, and so do the running checkpoints.
    fn follow(&mut self, history: kernel::CommitReadIncarnation) {
        if self.history != Some(history) {
            self.give_up_calls();
            self.history = Some(history);
        }
    }

    /// Gives up every session's judged calls and the history they stood
    /// in.
    fn give_up_calls(&mut self) {
        for entry in self.sessions.values_mut() {
            entry.calls.clear();
            entry.order.clear();
        }
        self.history = None;
    }

    /// The Kernel history the judged calls stand in, while it is the
    /// history `kernel` holds. Once a restore has installed another, the
    /// judged calls are given up, so a pass hands the window's results off
    /// again; a memo that holds no history holds no judged call. The
    /// comparison reads no row of the Kernel.
    fn history_in(
        &mut self,
        kernel: &kernel::KernelStore,
    ) -> Option<kernel::CommitReadIncarnation> {
        let held = self
            .history
            .filter(|history| kernel.require_incarnation(*history).is_ok());
        if held.is_none() {
            self.give_up_calls();
        }
        held
    }

    /// The Kernel history the judged calls stand in.
    #[cfg(test)]
    pub(crate) fn history_for_test(&self) -> Option<kernel::CommitReadIncarnation> {
        self.history
    }

    #[cfg(test)]
    pub(crate) fn has_seen_for_test(
        &self,
        scope: &str,
        session: &str,
        (entry, call): (&str, &str),
    ) -> bool {
        self.has_seen(session, call_digest(scope, entry, call))
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

    /// The id of the record an execution writes and the lineage of its
    /// command's records, for a test that places another row there.
    #[cfg(test)]
    pub(crate) fn ids_for_test(
        scope: &str,
        session: &str,
        (entry, call): (&str, &str),
        command: &str,
    ) -> (String, String) {
        let digest = command_named(scope, command).unwrap().digest;
        (
            record_id(scope, session, entry, call),
            format!("{FAILURE_SOURCE_ID}:{digest}"),
        )
    }
}

/// The command line a tool call carries: the value of `command` when it is
/// a string with a character other than whitespace, else that of `cmd`
/// when it is one. A shell tool of any name carries one. An array of
/// strings is the arguments of one program and no command line, so a call
/// that carries one alone carries no command line.
fn command_line(input: &Value) -> Option<&str> {
    ["command", "cmd"]
        .into_iter()
        .filter_map(|name| input.get(name)?.as_str())
        .find(|text| !text.trim().is_empty())
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

/// Whether `word` is a `NAME=value` assignment whose value is bare: a name
/// of ASCII letters, digits, and `_` that begins with no digit, `=`, and a
/// value that is empty or a plain word (see [`plain_word`]). A value with
/// any other character can hold a quote, an expansion, a redirection, an
/// operator that joins another command, or a mark of the redactor, and its
/// word stays a word of the command.
fn assigns(word: &str) -> bool {
    let name = word
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        .count();
    name > 0
        && !word.as_bytes()[0].is_ascii_digit()
        && word[name..]
            .strip_prefix('=')
            .is_some_and(|value| value.is_empty() || plain_word(value))
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
/// lines that differ inside the spans the redactor finds are one command
/// when the redactor writes the same marks for them.
///
/// The digest names the command line as it is written: the SHA-256 of the
/// project and the whole redacted line, each behind its length, and then,
/// for each finding of the redactor in order, the count of the command
/// line's bytes between the finding before it and that finding. Two
/// spellings of one operation, such as a run with and without a leading
/// `cd` or with other spacing, are two commands with a record each, so a
/// run of one spelling changes the record of that spelling alone. The
/// counts hold no byte and no length of a finding, so a credential of
/// another length under the same mark keeps the digest, and a command line
/// that spells the redactor's mark itself has another digest than one in
/// which the redactor wrote that mark.
///
/// The words shown (see [`shown_of`]) are a reading aid: the digest alone
/// names the command, and the record's `Command:` line and its rationale's
/// `command` field hold no other argument. The excerpt, and the path and
/// the error read from it, hold text of the output as the tool wrote it,
/// and a tool that prints its command there is quoted.
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

/// Whether a line of a failed command's output states an error: one of its
/// runs of letters and digits is one of [`ERROR_WORDS`] in any letter case
/// or ends in `Error` or `Exception` after a letter or digit, or the line
/// holds the shell's [`MISSING_COMMAND`] words. The rule chooses which
/// lines of a marked result to keep; it decides no failure.
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

/// Whether a line of a marked result, trimmed, can stand in a record: it is
/// not empty, and it holds no control character other than a tab and no
/// Unicode line separator.
fn shown(line: &str) -> bool {
    !line.is_empty() && printable(line) && !line.contains(['\u{2028}', '\u{2029}'])
}

/// What a record chooses from the text of a marked result, before the
/// redactor has judged that text.
struct Chosen {
    /// The exit code the harness states in the last non-empty line (see
    /// [`harness_exit_code`]), with the bytes of that line.
    stated: Option<(i64, Range<usize>)>,
    /// The lines that state an error, in output order, and then the closing
    /// lines that are none of them, each with the bytes it stands at. A
    /// text that several chosen lines share stands once, at its first line.
    lines: Vec<(Kept, Range<usize>)>,
    /// How many of `lines`, from the first, state an error.
    errors: usize,
    /// The text begins with the refusal heading (see [`begins_refusal`]).
    refusal: bool,
}

/// Whether a marked result's text begins with the heading of Pi's report of
/// a call whose arguments its schema refused. The record withholds the
/// output text when the heading matches.
fn begins_refusal(text: &str) -> bool {
    text.starts_with("Validation failed for tool \"")
}

/// The exit code that the last non-empty line of a marked result's whole
/// text states, which is where Pi's shell tool states it (see
/// [`harness_exit_code`]), with the line's place in the text. A result
/// that ends in another line states no code.
fn stated_in(text: &str) -> Option<(i64, Range<usize>)> {
    let mut end = text.len();
    for raw in text.rsplit('\n') {
        let span = end - raw.len()..end;
        let line = raw.trim();
        if !line.is_empty() {
            return harness_exit_code(line).map(|code| (code, span));
        }
        if span.start == 0 {
            break;
        }
        end = span.start - 1;
    }
    None
}

/// The lines a record chooses from the text of a marked result, and the
/// exit code the harness states. The code is read from the whole text (see
/// [`stated_in`]). When [`begins_refusal`] matches, the record withholds all
/// lines. Otherwise, the record selects lines from the full text.
///
/// The chosen lines are the first [`MAX_ERROR_LINES`] lines that state an
/// error, each failure line with the line that completes it, and then the
/// last [`MAX_CLOSING_LINES`] lines of the output. A line is read trimmed,
/// and one that cannot stand in a record (see [`shown`]) is left out. A
/// line of the output that states the command line, such as a shell's
/// trace of it or a runner's echo, is chosen like any other. A text that
/// several lines share is chosen once. The first pass ends once the error
/// lines are chosen, and the second reads the output from its end, so
/// neither holds a table of the output's lines.
fn chosen_of(text: &str) -> Chosen {
    let stated = stated_in(text);
    if begins_refusal(text) {
        return Chosen {
            stated,
            lines: Vec::new(),
            errors: 0,
            refusal: true,
        };
    }
    let mut lines: Vec<(Kept, Range<usize>)> =
        Vec::with_capacity(MAX_ERROR_LINES + MAX_CLOSING_LINES);
    let mut start = 0;
    let mut previous = "";
    for raw in text.split('\n') {
        if lines.len() == MAX_ERROR_LINES {
            break;
        }
        let span = start..start + raw.len();
        start = span.end + 1;
        let line = raw.trim();
        let states = shown(line)
            && (is_output_failure_line(line)
                || (is_output_failure_line(previous) && completes_failure_line(previous, line))
                || states_error(line));
        previous = line;
        if states {
            let text = clipped(line, MAX_LINE_BYTES);
            if lines.iter().all(|(kept, _)| kept.text != text) {
                let closing = false;
                lines.push((Kept { text, closing }, span));
            }
        }
    }
    let errors = lines.len();
    let mut end = text.len();
    let mut closing: Vec<(&str, Range<usize>)> = Vec::with_capacity(MAX_CLOSING_LINES);
    for raw in text.rsplit('\n') {
        let span = end - raw.len()..end;
        let line = raw.trim();
        if shown(line) {
            closing.push((line, span.clone()));
        }
        if span.start == 0 || closing.len() == MAX_CLOSING_LINES {
            break;
        }
        end = span.start - 1;
    }
    for (line, span) in closing.into_iter().rev() {
        let text = clipped(line, MAX_LINE_BYTES);
        // A closing line whose text an earlier chosen line holds marks that
        // line, so each text stands once.
        match lines.iter_mut().find(|(kept, _)| kept.text == text) {
            Some((earlier, _)) => earlier.closing = true,
            None => lines.push((
                Kept {
                    text,
                    closing: true,
                },
                span,
            )),
        }
    }
    Chosen {
        stated,
        lines,
        errors,
        refusal: false,
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

/// A marked result as a record states it, under the redactor's judgment
/// of its whole text.
///
/// The record keeps the exit code when it is nonzero and the redactor finds
/// nothing in its line.
///
/// The text kept follows source capture's rule for a tool span: text in
/// which the redactor finds nothing is kept as it is, and text with a
/// finding is withheld whole. With no finding the chosen lines stand in
/// the record. With a finding the record holds no line. The path and the
/// error name come from the chosen lines that state an error and hold no
/// finding; when the scan does not finish, no text of the output reaches
/// the record.
fn screened(text: &str) -> Failure {
    screened_with(text, findings_in)
}

/// [`screened`] with `scan` as the redactor: it returns the bytes of its
/// findings in the text it is given, or `None` when its scan did not
/// finish. The lines are chosen from, the scan runs on, and the names are
/// read from the one `text`.
fn screened_with(text: &str, scan: impl FnOnce(&str) -> Option<Vec<Range<usize>>>) -> Failure {
    let Chosen {
        stated,
        lines,
        errors,
        refusal,
    } = chosen_of(text);
    let findings = scan(text);
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
    // The first name among the lines that state an error and hold no
    // finding, read from the line as the output holds it.
    let named = |name: fn(&str) -> Option<&str>| {
        lines[..errors]
            .iter()
            .filter(|(_, span)| clear(span))
            .find_map(|(_, span)| name(&text[span.clone()]))
            .map(str::to_owned)
    };
    let (path, error) = (named(named_path), named(named_error));
    let excerpt = match findings.as_deref() {
        Some([]) if refusal => Excerpt::Refusal,
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
/// after it is `:` or `(`, a digit, and then digits, `:`, `,`, and `)`; a
/// test's name follows `::` in letters, digits, `_`, and `:`. Quotes,
/// brackets, and closing punctuation around the word are left out. The
/// reading is by shape: a host with a port has the shape of a file with a
/// line.
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
            None => {
                rest.is_empty()
                    || (rest.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
                        && rest
                            .bytes()
                            .skip(1)
                            .all(|byte| byte.is_ascii_digit() || b":,)".contains(&byte)))
            }
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

/// A word of a line that may name an error: a run of letters, combining
/// marks, and numeric characters of any script, `_`, and `.`. A mark
/// belongs to the letter it follows, so a name spelled with one is one
/// word.
static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{L}\p{M}\p{N}_.]+").expect("the word pattern compiles"));

/// A word that is a name: letters, combining marks, decimal digits, `_`,
/// and `.` alone.
static NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\p{L}[\p{L}\p{M}\p{Nd}_.]*$").expect("the name pattern compiles")
});

/// The error a line names: its first word that is a type name ending in
/// `Error` or `Exception` (`ModuleNotFoundError`,
/// `java.lang.NullPointerException`) or a diagnostic code of one to four
/// ASCII capitals and two to six ASCII digits (`E0308`, `TS2307`). A word
/// is read whole (see [`WORD`]), without the dots at its ends, and is named
/// whole or left out: a type name holds letters, marks, decimal digits,
/// `_`, and `.` and begins with a letter, so a name with a letter outside
/// ASCII or a combining mark is one name, and a word with another numeric
/// character, such as a fraction, names no error. The reading is by shape:
/// the name of a digest such as `SHA256` has the shape of a code.
fn named_error(line: &str) -> Option<&str> {
    WORD.find_iter(line)
        .map(|word| word.as_str().trim_matches('.'))
        .find(|word| {
            let typed = word
                .strip_suffix("Error")
                .or_else(|| word.strip_suffix("Exception"))
                .is_some_and(|stem| !stem.is_empty())
                && NAME.is_match(word);
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
/// [`RESOLVED_NOTE`]: the time of the result and what the harness reports
/// of it, the command's name, the path and the error the output's error
/// lines name, and the output's chosen lines, each behind [`QUOTE_MARK`],
/// or the sentence that says why they are withheld and where the result's
/// text stands. The first line states the harness's mark and the exit code
/// the result's last line states; it claims no more of the command, since
/// the harness marks a call it refused or ended as it marks a failed one. The time stands
/// in the first line, so a reader of a search result that is cut still
/// orders the failures of a project. The limits of the parts leave room for
/// the first two lines and for the closing lines or the withheld sentence
/// whole. The path and the error stand in the text when they fit beside
/// those, and the lines that state an error fill the bytes that remain; the
/// rationale holds the path and the error in every case.
fn summary_of(unit: &Observed, session: &str, failure: &Failure) -> String {
    let stated = match failure.exit_code {
        Some(code) => format!(
            "the harness marked the result as an error and its last line states exit code {code}"
        ),
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
    let withheld = |reason: &str| {
        format!(
            "\nOutput text withheld: {reason}. The text stands in the result of tool call {} of session {} while that session's history holds it.",
            clipped(&unit.call, MAX_ID_BYTES),
            clipped(session, MAX_ID_BYTES),
        )
    };
    let (lines, ending) = match &failure.excerpt {
        Excerpt::Kept(lines) => (lines.as_slice(), "\nOutput lines:".to_owned()),
        Excerpt::Finding => (
            &[][..],
            withheld("the redactor found credential-shaped content in the output"),
        ),
        Excerpt::Unscanned => (
            &[][..],
            withheld("the redactor's scan of the output did not finish"),
        ),
        Excerpt::Refusal => (
            &[][..],
            withheld(
                "the output begins with the heading of the harness's report of a refused call",
            ),
        ),
    };
    let mut reserved: usize = ending.len()
        + lines
            .iter()
            .filter(|line| line.closing)
            .map(cost)
            .sum::<usize>();
    for (label, name) in [
        ("Path named", &failure.path),
        ("Error named", &failure.error),
    ] {
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
    /// The Kernel history under which the memo's judged calls filtered the
    /// window: `None` while the memo held none.
    history: Option<kernel::CommitReadIncarnation>,
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

/// Whether `value` is an id a Pi row carries itself: a string with a
/// character other than whitespace.
fn carried(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
}

/// Whether the Pi row `row` stands for a message entry of the session as
/// the session holds it. The Pi plugin sets a row's `type` to its entry's
/// when the row's message is that entry's message, so a message that a
/// context handler changed, and one whose entry the plugin cannot read,
/// carry no entry's identity. Source capture reads a row by the same field.
fn entry_row(row: &Value) -> bool {
    row.get("type").and_then(Value::as_str) == Some("message")
}

/// The tool call parts of the Pi row `row`, in the order of its content,
/// which is the order of the tool call blocks the codec derives from it.
fn native_call_parts(row: &Value) -> impl Iterator<Item = &Value> {
    row.get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("toolCall"))
}

/// The id the tool call part `part` of a Pi row carries itself, by the
/// codec's precedence: its `id` when that is a string, else its `callId`.
/// The codec gives a call part without one a synthesized id, which names
/// no execution.
fn native_call_id(part: &Value) -> Option<&str> {
    match part.get("id") {
        Some(Value::String(_)) => carried(part.get("id")),
        _ => carried(part.get("callId")),
    }
}

/// The `toolCallId` the Pi row `row` of a tool result carries itself. The
/// codec gives a result row without one the fixed id `tool`, which names
/// no execution.
fn native_result_id(row: &Value) -> Option<&str> {
    let message = row.get("message")?;
    (message.get("role").and_then(Value::as_str) == Some("toolResult"))
        .then(|| carried(message.get("toolCallId")))
        .flatten()
}

/// A command call that waits for its result.
struct Waiting<'a> {
    tool: &'a str,
    command: &'a str,
}

/// The tool turn under way while a window's results are paired.
#[derive(Default)]
struct Turn<'a> {
    /// The command calls no result has answered yet, by the call's id.
    calls: HashMap<&'a str, Waiting<'a>>,
    /// The ids the turn's calls and results carry, and those that two
    /// calls or two results of the turn carry.
    named: HashSet<&'a str>,
    answered: HashSet<&'a str>,
    shared: HashSet<&'a str>,
    /// The results the turn pairs with a command call, in window order.
    paired: Vec<Candidate<'a>>,
}

impl<'a> Turn<'a> {
    /// Ends the turn: its pairs under an id the turn carries once join
    /// `kept`, which holds the last [`MEMO_CALLS_PER_SESSION`] of them.
    fn close(&mut self, kept: &mut VecDeque<Candidate<'a>>) {
        for candidate in self.paired.drain(..) {
            if !self.shared.contains(candidate.call) {
                kept.push_back(candidate);
                if kept.len() > MEMO_CALLS_PER_SESSION {
                    kept.pop_front();
                }
            }
        }
        *self = Self::default();
    }
}

/// The command results of a window, in window order: each result paired
/// with the call it answers in its tool turn, which is a command call of
/// the assistant message before it, with no message of another role
/// between them. A tool turn begins at an assistant message and ends at
/// the next assistant message, which begins another, or at the next
/// message that is neither the assistant's nor a tool's. A result under an
/// id that two tool calls or two tool results of its turn carry is left
/// out, since the id names neither alone; an id that another turn uses
/// names another execution there. The last [`MEMO_CALLS_PER_SESSION`] of
/// the remaining pairs are kept.
///
/// `native` holds the Pi rows the window's messages were derived from, one
/// row for each message in order. A call or a result counts when its row
/// stands for a message entry of the session (see [`entry_row`]) and
/// carries the id itself: for a call, the tool call part at the block's
/// place among the row's tool call parts (see [`native_call_id`]), and for
/// a result, the row's `toolCallId` (see [`native_result_id`]). Any other
/// call is answered by no result, and any other result changes no record
/// and settles no call.
///
/// `None` for a window that names no executions: one in which two messages
/// share an entry id or an entry id passes [`crate::wire::MAX_MID_BYTES`],
/// which the transform refuses too, and one whose rows differ from its
/// messages in number or in an id.
fn command_results<'a>(
    messages: &'a [Arc<crate::wire::IngressMessage>],
    native: &'a [Arc<Value>],
) -> Option<VecDeque<Candidate<'a>>> {
    let mut entries = HashSet::with_capacity(messages.len());
    let derived = native.len() == messages.len()
        && native.iter().zip(messages).all(|(row, message)| {
            row.get("id").and_then(Value::as_str) == Some(message.mid.as_str())
                && message.mid.len() <= crate::wire::MAX_MID_BYTES
                && entries.insert(message.mid.as_str())
        });
    if !derived {
        return None;
    }
    let mut kept = VecDeque::new();
    let mut turn = Turn::default();
    for (message, row) in messages.iter().zip(native) {
        let role = message.ck.role.as_str();
        if role == "assistant" {
            turn.close(&mut kept);
        }
        let held = entry_row(row);
        // Each tool call block takes the next tool call part of its row.
        let mut parts = native_call_parts(row);
        for block in message.ck.content() {
            match block.kind() {
                BlockKind::ToolCall {
                    id, name, input, ..
                } => {
                    let id = execution_id(block, id);
                    let carried = parts.next().and_then(native_call_id);
                    if !held || carried != Some(id) {
                        continue;
                    }
                    if !turn.named.insert(id) {
                        turn.shared.insert(id);
                    }
                    if let Some(command) = command_line(input).filter(|_| role == "assistant") {
                        let tool = name.as_str();
                        turn.calls.insert(id, Waiting { tool, command });
                    }
                }
                BlockKind::ToolResult { id, output, .. } => {
                    let call = execution_id(block, id);
                    if !held || native_result_id(row) != Some(call) {
                        continue;
                    }
                    if !turn.answered.insert(call) {
                        turn.shared.insert(call);
                    }
                    if let Some(Waiting { tool, command }) = turn.calls.remove(call) {
                        turn.paired.push(Candidate {
                            entry: message.mid.as_str(),
                            call,
                            tool,
                            command,
                            output: &output.kind,
                            at_ms: message.ck.meta.created_at_ms,
                        });
                    }
                }
                _ => {}
            }
        }
        // A message of another role ends the tool turn: a result after it
        // answers no call of that turn and shares no id with it.
        if role != "assistant" && role != "tool" {
            turn.close(&mut kept);
        }
    }
    turn.close(&mut kept);
    Some(kept)
}

impl HandlerCore {
    /// Copies of the command results of the window that this session's
    /// memo has not judged (see [`command_results`]), for a primary Pi
    /// session whose configuration permits memory capture and consents to
    /// source capture: a record is a durable copy of part of a tool result,
    /// which source capture retains under that consent. The consent read is
    /// the one the session's route saved when it was bound. The pass hands
    /// off the earliest [`MAX_RESULTS_PER_PASS`] unjudged results whose
    /// copies fit [`MAX_HANDOFF_BYTES`] together. Each result is measured
    /// where it stands in the window and copied once it fits (see
    /// [`Candidate::readable`]), so every single result fits, and a result
    /// that is not read is handed off by name alone and settled unread. The
    /// task holds no message of the pass. One checkpoint of a session runs
    /// at a time, and [`MAX_CHECKPOINTS`] over all sessions. The memo
    /// filters the window while the Kernel holds the history its judged
    /// calls stand in (see [`FailureMemo::history_in`]), and the task
    /// judges under that history. Returns `None` when the session's id
    /// passes the transform's byte bound, when the request carries no
    /// native rows, when the window names no executions, and when it holds
    /// no unjudged result, so the caller spawns no task.
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
        let kernel = self.kernel.kernel_store().ok()?;
        let scope = binding.kernel_project.digest();
        // The transform refuses a session id past its byte bound.
        if binding.session.len() > crate::transform::MAX_REQUEST_IDENTITY_BYTES {
            return None;
        }
        // A Pi window reaches the daemon as the session's rows.
        let native = request.native_messages.as_deref()?;
        let mut considered = command_results(&request.messages, native)?;
        let digest =
            |candidate: &Candidate<'_>| call_digest(scope, candidate.entry, candidate.call);
        let (epoch, history) = {
            let mut memo = self.failure_memo.lock().expect("failure memo mutex");
            if memo.busy(&binding.session) {
                return None;
            }
            // The judged calls filter the window while the Kernel holds
            // the history they stand in.
            let history = memo.history_in(&kernel);
            considered.retain(|candidate| !memo.has_seen(&binding.session, digest(candidate)));
            if considered.is_empty() {
                return None;
            }
            (memo.pin(&binding.session), history)
        };
        let pin = MemoPin {
            memo: Arc::clone(&self.failure_memo),
            session: binding.session.clone(),
            epoch,
        };
        let mut pending = Vec::new();
        let mut copied = 0;
        for candidate in considered.into_iter().take(MAX_RESULTS_PER_PASS) {
            let readable = candidate.readable();
            let bytes = readable.as_ref().map_or(0, Readable::bytes);
            if copied + bytes > MAX_HANDOFF_BYTES {
                break;
            }
            copied += bytes;
            pending.push(Pending {
                digest: digest(&candidate),
                input: readable.as_ref().map(Readable::copied),
            });
        }
        Some(FailureCheckpoint {
            kernel: Arc::clone(&self.kernel),
            commit_gate: Arc::clone(&self.capture_commit_gate),
            project: binding.kernel_project.clone(),
            pin,
            pending,
            history,
            cancel: self.cancel.clone(),
        })
    }
}

impl<'a> Candidate<'a> {
    /// The fields of a result that is read, as they stand in the window, or
    /// `None` for a result that changes no record whatever it holds: one
    /// whose message carries no creation time or a time outside the years
    /// the calendar renders, which has no place among the command's runs,
    /// one whose entry id, call id, or tool name is empty or passes
    /// [`MAX_NAME_BYTES`], one that is no text or whose text passes
    /// [`MAX_OUTPUT_BYTES`] (see [`text_of`]), and one whose command line
    /// passes that bound. An unmarked result is read for the shape of its
    /// text alone.
    fn readable(&self) -> Option<Readable<'a>> {
        let at_ms = self.at_ms.filter(|at_ms| utc_time(*at_ms).is_some())?;
        let names = [self.entry, self.call, self.tool];
        if names
            .iter()
            .any(|name| name.is_empty() || name.len() > MAX_NAME_BYTES)
        {
            return None;
        }
        let (text, marked) = text_of(self.output)?;
        Some(Readable {
            entry: self.entry,
            call: self.call,
            tool: self.tool,
            command: Some(self.command).filter(|command| command.len() <= MAX_OUTPUT_BYTES)?,
            marked: marked.then_some(text),
            at_ms,
        })
    }
}

impl Readable<'_> {
    /// The bytes a copy of these fields holds.
    fn bytes(&self) -> usize {
        self.entry.len()
            + self.call.len()
            + self.tool.len()
            + self.command.len()
            + self.marked.map_or(0, str::len)
    }

    fn copied(&self) -> Input {
        Input {
            entry: self.entry.to_owned(),
            call: self.call.to_owned(),
            tool: self.tool.to_owned(),
            command: self.command.to_owned(),
            marked: self.marked.map(str::to_owned),
            at_ms: self.at_ms,
        }
    }
}

impl Input {
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
        let outcome = match &self.marked {
            Some(text) => Outcome::Failed(screened(text)),
            None => Outcome::Passed,
        };
        Ok(Observed {
            entry: self.entry.clone(),
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
/// busy or failing, a restore that failed has closed its handle, another
/// writer changed a row under the judgment, or a read named a commit past
/// the tip, as it does once a restore has installed an earlier history. Any
/// other refusal would meet the same result again.
const fn leaves_for_later(error: &kernel::KernelError) -> bool {
    use kernel::KernelError::{
        Busy, Deadline, Fault, FenceLost, FutureSnapshot, Held, InvalidRestore, Io, NotFound,
    };
    matches!(
        error,
        Deadline
            | Busy
            | Held
            | Io
            | FenceLost
            | Fault
            | NotFound
            | FutureSnapshot
            | InvalidRestore
    )
}

/// What judging one command result did.
#[derive(Debug, PartialEq, Eq)]
enum Judged {
    Wrote,
    Nothing,
    /// The result writes nothing for a reason an operator can act on.
    Left(&'static str),
    /// A restore installed another Kernel history between the judgment's
    /// reads and its commit, which then wrote nothing. The call is left for
    /// a later pass, which judges it against the history the Kernel holds.
    Restored,
}

/// The Kernel history a judgment reads: `known` while it is the history the
/// Kernel holds, else the one a fresh read of the Kernel's tip names.
fn history_of(
    kernel: &kernel::KernelStore,
    known: Option<kernel::CommitReadIncarnation>,
    budget: &EvalBudget,
) -> Result<kernel::CommitReadIncarnation, kernel::KernelError> {
    match known {
        Some(history) if kernel.require_incarnation(history).is_ok() => Ok(history),
        _ => Ok(kernel
            .capture_commit_read_target_within_budget(budget)?
            .incarnation),
    }
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
    /// gate is taken by polling, so a shutdown ends the wait for it. Each
    /// judgment first reads which history the Kernel holds: the memo's
    /// calls stand in one history, and under another they are given up
    /// (see [`FailureMemo::follow`]). The pass judges under one history,
    /// the one its window was filtered under or the first one it reads,
    /// and ends when the Kernel holds another. A history that cannot be
    /// read, a Kernel that is busy or failing, a row another writer changed
    /// under the judgment, and a restore between the judgment's reads and
    /// its commit end the pass with the call unlearned (see
    /// [`leaves_for_later`], [`Judged::Restored`]), so a later pass whose
    /// window still carries the call and its result judges it and the
    /// results after it again, in window order. Any other refusal of the
    /// Kernel to a judgment settles the call. A panic under the gate leaves the gate usable: the guard is
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
            history: mut pass_history,
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
            #[cfg(test)]
            before_commit: pin
                .memo
                .lock()
                .expect("failure memo mutex")
                .before_commit
                .clone(),
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
                let budget =
                    EvalBudget::new(Some(Instant::now() + COMMIT_WAIT), Arc::clone(interrupt));
                // The memo's calls were judged against one Kernel history.
                // The memo is read and released ahead of the Kernel read.
                let known = memo().history;
                let history = match history_of(&kernel, known, &budget) {
                    Ok(history) => history,
                    Err(error) => return Some((Err(error), false)),
                };
                memo().follow(history);
                // The pass judges under one history: the one its window was
                // filtered under, or the first one a judgment of it reads.
                if *pass_history.get_or_insert(history) != history {
                    return Some((Ok(Judged::Restored), false));
                }
                if memo().has_seen(&pin.session, result.digest) {
                    return Some((Ok(Judged::Nothing), true));
                }
                #[cfg(test)]
                {
                    let hook = memo().under_gate.clone();
                    if let Some(hook) = hook {
                        hook();
                    }
                }
                let judged = match &unit {
                    Ok(unit) => writer.judge(unit, history, &budget),
                    Err(Some(reason)) => Ok(Judged::Left(reason)),
                    Err(None) => Ok(Judged::Nothing),
                };
                let settles = match &judged {
                    Ok(Judged::Restored) => false,
                    Ok(_) => true,
                    Err(error) => !leaves_for_later(error),
                };
                // A settled call is learned while the memo stands in the
                // history its judgment read; the check and the insert share
                // one hold of the memo.
                let learned = match &judged {
                    // The calls judged so far stand in the replaced history.
                    Ok(Judged::Restored) => {
                        memo().give_up_calls();
                        false
                    }
                    _ => settles && memo().learn(&pin.session, pin.epoch, result.digest, history),
                };
                Some((judged, learned))
            }));
            drop(gate);
            let (judged, learned) = match judged {
                Err(panic) => std::panic::resume_unwind(panic),
                Ok(None) => break,
                Ok(Some(judgment)) => judgment,
            };
            match judged {
                Ok(Judged::Wrote) => written += 1,
                Ok(Judged::Nothing | Judged::Restored) => {}
                Ok(Judged::Left(reason)) => left(&reason),
                Err(error) => left(&error),
            }
            // A call left unlearned ends the pass, so a later pass judges
            // it and the results after it in window order.
            if !learned {
                break;
            }
        }
        written
    }
}

/// The id of the record an execution writes. Each part is hashed behind
/// its length, so two executions share an id only when project, session,
/// the entry that holds the result, and the call id are equal.
fn record_id(scope: &str, session: &str, entry: &str, call: &str) -> String {
    let mut hash = Sha256::new();
    for part in [FAILURE_SOURCE_ID, scope, session, entry, call] {
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
        Excerpt::Finding | Excerpt::Unscanned | Excerpt::Refusal => "withheld",
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

/// The row one judgment writes.
struct Row<'a> {
    /// The record's id, which is the decision's object id.
    id: String,
    /// The operation key the Kernel logs the commit under.
    key: String,
    /// The command's lineage, and the revision the row takes on it.
    lineage: &'a str,
    revision: i64,
    /// The live row the new one takes the place of.
    target: Option<&'a str>,
    payload: kernel::DecisionPayload,
}

/// The Kernel, the project, and the session one checkpoint writes for.
struct Writer<'a> {
    kernel: &'a kernel::KernelStore,
    project: &'a ProjectBinding,
    /// The project's digest, which scopes a record's id and a command's
    /// lineage.
    scope: &'a str,
    session: &'a str,
    /// Runs between a judgment's Kernel reads and its commit.
    #[cfg(test)]
    before_commit: Option<Arc<dyn Fn() + Send + Sync>>,
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
    ///
    /// The reads belong to the Kernel history `history`, which the caller
    /// read before them. The commit requires that history while it holds
    /// the Kernel's writer, so a judgment whose reads a restore has
    /// replaced writes nothing: it answers [`Judged::Restored`], or
    /// [`Judged::Nothing`] when the history the Kernel now holds has this
    /// execution's receipt.
    fn judge(
        &self,
        unit: &Observed,
        history: kernel::CommitReadIncarnation,
        budget: &EvalBudget,
    ) -> Result<Judged, kernel::KernelError> {
        let id = record_id(self.scope, self.session, &unit.entry, &unit.call);
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
        #[cfg(test)]
        if let Some(hook) = &self.before_commit {
            hook();
        }
        let row = Row {
            id,
            key,
            lineage: &lineage,
            revision,
            target,
            payload,
        };
        self.commit(row, history, budget)
    }

    /// Commits `row`: the record under its operation key at its revision
    /// of its lineage, in place of the live row it names when it names one,
    /// as a labeled row. The Kernel stores the text and the rationale
    /// through the durable-text redactor; a payload the redactor would
    /// change writes no record, so the stored row is the row composed here.
    /// The record's id is new to the Kernel: a row another writer holds
    /// under it stays as it is, and the call is settled unwritten.
    /// The commit writes while the Kernel holds the history `history`: the
    /// check runs under the Kernel's writer, where no restore can follow
    /// it. A commit the Kernel answers from an earlier receipt wrote
    /// nothing now and runs no check: the receipt stands in the history
    /// the Kernel holds, which thereby holds this execution's commit.
    fn commit(
        &self,
        row: Row<'_>,
        history: kernel::CommitReadIncarnation,
        budget: &EvalBudget,
    ) -> Result<Judged, kernel::KernelError> {
        let Row {
            id,
            key,
            lineage,
            revision,
            target,
            payload,
        } = row;
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
        // What the commit's operation refuses, and how the call is judged
        // then.
        let refused = std::cell::Cell::new(None);
        let receipt = self
            .kernel
            .commit_within_budget(budget, intent, |envelope| {
                let refuse = |judged| {
                    refused.set(Some(judged));
                    Err(kernel::KernelError::Conflict)
                };
                if self.kernel.require_incarnation(history).is_err() {
                    return refuse(Judged::Restored);
                }
                // The record's id names this execution alone. A row that
                // another writer holds under it stays as it is.
                if envelope.object_state(&id)?.is_some() {
                    return refuse(Judged::Left("another row holds the record's id"));
                }
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
            });
        if let Some(judged) = refused.take() {
            return Ok(judged);
        }
        Ok(match receipt?.replayed {
            true => Judged::Nothing,
            false => Judged::Wrote,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCOPE: &str = "project";

    /// The exit code and the kept lines of a marked text result of `make`.
    fn failed(text: &str) -> (Option<i64>, Vec<String>) {
        let failure = screened(text);
        let lines = match failure.excerpt {
            Excerpt::Kept(lines) => lines.into_iter().map(|line| line.text).collect(),
            Excerpt::Finding | Excerpt::Unscanned | Excerpt::Refusal => Vec::new(),
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

    /// The task's copy of a text result of `command`, read from the
    /// window as a pass reads it.
    fn input(command: &str, text: &str, flagged: bool) -> Input {
        let text = text.to_owned();
        let output = match flagged {
            true => OutputKind::ErrorText { text },
            false => OutputKind::Text { text },
        };
        let candidate = Candidate {
            entry: "entry-1",
            call: "call-1",
            tool: "bash",
            command,
            output: &output,
            at_ms: Some(1_000),
        };
        let readable = candidate.readable().expect("a result within the bounds");
        readable.copied()
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
    fn a_command_line_is_read_from_the_call() {
        assert_eq!(
            command_line(&json!({ "command": "cargo test" })),
            Some("cargo test")
        );
        assert_eq!(
            command_line(&json!({ "command": "  ", "cmd": "npm run build" })),
            Some("npm run build")
        );
        // An array of strings is the arguments of one program and no
        // command line.
        assert_eq!(
            command_line(&json!({ "command": ["npm", "run"], "cmd": "make" })),
            Some("make")
        );
        for input in [
            json!({ "path": "src/lib.rs" }),
            json!({ "command": "" }),
            json!({ "command": ["npm", "run", "build"] }),
            json!({ "cmd": ["sh", "-c", "make | tail"] }),
            json!({ "command": 7 }),
        ] {
            assert_eq!(command_line(&input), None, "{input}");
        }
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
            // An operator written against an assignment joins another
            // command, whose words are no operands of this one.
            ("A=1&&make test", LEFT_OUT),
            ("FOO=a;ls -la /x", LEFT_OUT),
            ("FOO=a|tee out.log", LEFT_OUT),
            ("VAR=(a b c) make", LEFT_OUT),
            ("PATH=/opt/bin:/usr/bin make", LEFT_OUT),
            ("TZ= date", "date"),
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
        // under one mark are one command, and the credential reaches no
        // word shown.
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
        // The digest holds the gap ahead of each finding. Two findings on
        // one line: a first credential of another length moves the second
        // finding and keeps both gaps, so the command is the same.
        let redacted = |command: &str| {
            context_core::redaction::redact_windowed_durable_text(command, usize::MAX).unwrap()
        };
        let second = ["s3c0nd", "-fixture", "-value-", "7731"].concat();
        let twice = |first: &str| {
            format!(
                "deploy --{first} --region eu --{}={second} --target api",
                ["access_", "tok", "en"].concat()
            )
        };
        let longer = assignment.replace(&value, &format!("{value}-and-nine-more"));
        for command in [twice(&assignment), twice(&longer)] {
            assert_eq!(redacted(&command).detections.len(), 2, "{command}");
        }
        assert_ne!(
            redacted(&twice(&assignment)).detections[1].offset,
            redacted(&twice(&longer)).detections[1].offset,
            "the second finding moved"
        );
        assert_eq!(digest(&twice(&assignment)), digest(&twice(&longer)));
        // One finding and one spelled mark in either order: the redacted
        // lines are equal with one finding each, at another place, and the
        // commands are two.
        let token = bare_token();
        let mark = redacted(&token).text;
        assert_eq!(redacted(&mark).detections.len(), 0);
        let (found_first, found_last) = (
            format!("deploy {token} {mark}"),
            format!("deploy {mark} {token}"),
        );
        assert_eq!(redacted(&found_first).text, redacted(&found_last).text);
        assert_eq!(
            (
                redacted(&found_first).detections.len(),
                redacted(&found_last).detections.len()
            ),
            (1, 1)
        );
        assert_ne!(digest(&found_first), digest(&found_last));
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
            "Working directory does not exist: /srv/gone\nCannot execute bash commands.",
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

    /// Pi answers a call whose arguments its schema refuses with a report
    /// under the heading `Validation failed for tool`. The record of a text
    /// that begins with the heading names the heading as the reason and
    /// withholds the lines, the path, and the error.
    #[test]
    fn the_text_of_a_refused_call_s_report_is_withheld() {
        let unruled = ["-p", "hunter", "2-fixture"].concat();
        let command = format!("deploy /srv/app/config.yaml {unruled}");
        let arguments = format!(
            "{{\n  \"command\": {},\n  \"timeout\": \"TypeError at src/check.ts:10 failed\"\n}}",
            Value::from(command.as_str())
        );
        let text = format!(
            "Validation failed for tool \"bash\":\n  - timeout: must be number\n\nReceived arguments:\n{arguments}"
        );
        assert_eq!(
            kernel::exact_text_refusal(&text),
            None,
            "the redactor passes the fixture"
        );
        // Pi's report; one whose diagnostic holds the arguments' heading as
        // a line of its own; one with CRLF line ends; one with text after
        // the arguments.
        let reports = [
            text.clone(),
            text.replacen(
                "  - timeout: must be number",
                "  - /env/a\nReceived arguments:\nb: must be string",
                1,
            ),
            text.replace('\n', "\r\n"),
            format!("{text}\n\nRetry with a number."),
        ];
        for report in &reports {
            let failure = recorded(&command, report, true).unwrap();
            assert_eq!(
                failure,
                Failure {
                    exit_code: None,
                    path: None,
                    error: None,
                    excerpt: Excerpt::Refusal,
                },
                "{report}"
            );
            let unit = input(&command, report, true).observe(SCOPE).unwrap();
            let record = failed_record("s1", &unit, &failure);
            assert!(
                record.summary.ends_with(
                    "\nCommand: deploy /srv/app/config.yaml [arguments left out]\nOutput text withheld: the output begins with the heading of the harness's report of a refused call. The text stands in the result of tool call call-1 of session s1 while that session's history holds it."
                ),
                "{}",
                record.summary
            );
            for stored in [&record.summary, &record.rationale] {
                for argument in [unruled.as_str(), "TypeError", "must be", "/env/a", "Retry"] {
                    assert!(!stored.contains(argument), "{stored}");
                }
            }
            assert_eq!(
                serde_json::from_str::<Value>(&record.rationale).unwrap()["output_text"],
                "withheld"
            );
        }
        let kept = |failure: &Failure| match &failure.excerpt {
            Excerpt::Kept(lines) => lines.iter().map(|line| line.text.clone()).collect(),
            _ => Vec::new(),
        };
        // The positive control: the same lines behind another first line, as
        // the output of a command that ran, are read whole.
        let ran = format!("deploy says:\n{text}\n\nCommand exited with code 2");
        let failure = recorded(&command, &ran, true).unwrap();
        assert_eq!(failure.exit_code, Some(2));
        assert_eq!(failure.path.as_deref(), Some("src/check.ts:10"));
        assert_eq!(failure.error.as_deref(), Some("TypeError"));
        assert!(
            kept(&failure)
                .iter()
                .any(|line: &String| line.contains("TypeError"))
        );
        // The redactor judges the whole text of the report: a credential
        // among the arguments is the reason the record states.
        let (assignment, value) = keyed_credential();
        let secret = text.replace(&unruled, &assignment);
        let failure = recorded(&command.replace(&unruled, &assignment), &secret, true).unwrap();
        assert_eq!(
            failure,
            Failure {
                exit_code: None,
                path: None,
                error: None,
                excerpt: Excerpt::Finding,
            }
        );
        assert!(!format!("{failure:?}").contains(&value));
        // A scan that does not finish is the reason such a record states.
        assert_eq!(
            screened_with(&text, |_| None),
            Failure {
                exit_code: None,
                path: None,
                error: None,
                excerpt: Excerpt::Unscanned,
            }
        );
        // The heading ends at the quote that opens the tool's name: a text
        // without that quote, and one with a shorter beginning, are read
        // whole, and the heading alone is withheld.
        for near in [
            "Validation failed for tool bash:\n  - TypeError at src/check.ts:10 failed",
            "Validation failed: TypeError at src/check.ts:10 failed",
        ] {
            let failure = recorded("make", near, true).unwrap();
            assert_eq!(failure.path.as_deref(), Some("src/check.ts:10"), "{near}");
            assert_eq!(failure.error.as_deref(), Some("TypeError"), "{near}");
            assert!(matches!(failure.excerpt, Excerpt::Kept(_)), "{near}");
        }
        let bare = "Validation failed for tool \"bash: TypeError at src/check.ts:10 failed";
        assert_eq!(
            recorded("make", bare, true).unwrap(),
            Failure {
                exit_code: None,
                path: None,
                error: None,
                excerpt: Excerpt::Refusal,
            }
        );
    }

    /// A line states an error when one of its runs of letters and digits
    /// is an error word, so a word joined to others by `_` or `.` counts.
    #[test]
    fn an_error_word_inside_an_identifier_makes_its_line_an_error_line() {
        for line in ["retry error_count=3", "hook on.failure ran", "FAILED."] {
            let text = format!("first plain line\n{line}\nplain one\nplain two");
            let chosen = chosen_of(&text);
            assert_eq!(chosen.errors, 1, "{line}");
            assert_eq!(chosen.lines[0].0.text, line);
            assert!(!chosen.lines[0].0.closing, "{line}");
        }
        let plain = chosen_of("first plain line\nretry errorcount=3\nplain one\nplain two");
        assert_eq!(plain.errors, 0);
    }

    /// The exit code is the one the last non-empty line of the whole
    /// result states, also of a text that begins as a refusal report: a
    /// command's own output that begins with the heading and ends in the
    /// harness's status line keeps that line's code and withholds its lines.
    #[test]
    fn a_command_s_output_that_begins_as_a_refusal_report_keeps_its_own_exit_code() {
        // The output states a code in an earlier line and ends in the
        // harness's status line.
        let printed = "Validation failed for tool \"x\":\nCommand exited with code 7\n\nReceived arguments:\n{}\n\nCommand exited with code 2";
        assert_eq!(failed(printed), (Some(2), Vec::new()));
        let unit = input("make", printed, true).observe(SCOPE).unwrap();
        let Outcome::Failed(failure) = &unit.outcome else {
            panic!("a marked result");
        };
        assert_eq!(failure.excerpt, Excerpt::Refusal);
        let record = failed_record("s1", &unit, failure);
        assert!(
            record.summary.contains("its last line states exit code 2"),
            "{}",
            record.summary
        );
        assert_eq!(
            serde_json::from_str::<Value>(&record.rationale).unwrap()["exit_code"],
            2
        );
        // The harness's own report ends in its arguments and states no
        // code, also when an earlier line holds one.
        let refused = "Validation failed for tool \"bash\":\nCommand exited with code 7\n\nReceived arguments:\n{\n  \"command\": \"make\"\n}\n";
        assert_eq!(failed(refused), (None, Vec::new()));
        // A code of 0 and a last line of other words state no code.
        assert_eq!(
            stated_in("error: boom\nCommand exited with code 0").map(|(code, _)| code),
            Some(0)
        );
        assert_eq!(failed("error: boom\nCommand exited with code 0").0, None);
        assert_eq!(
            stated_in("Command exited with code 4\nerror: boom\n\n"),
            None
        );
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
        // A path and an error are named by the lines that state an error:
        // the log path of the note and a name in a closing line name none.
        let unnamed = format!("error: boom\nwork\n{note}\nsee the notes on E0308");
        let failure = recorded("make", &unnamed, true).unwrap();
        let Excerpt::Kept(lines) = &failure.excerpt else {
            panic!("{failure:?}");
        };
        assert_eq!(
            lines.iter().map(|line| &*line.text).collect::<Vec<_>>(),
            ["error: boom", note, "see the notes on E0308"]
        );
        assert_eq!((failure.path, failure.error), (None, None));
        let named = format!("KeyError: 'port' in conf/app.yaml:3\nwork\n{note}\nsee E0308");
        let failure = recorded("make", &named, true).unwrap();
        assert_eq!(
            (failure.path.as_deref(), failure.error.as_deref()),
            (Some("conf/app.yaml:3"), Some("KeyError"))
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
        // Two lines that differ past the bytes kept are one text, which
        // stands at the first of them: the second, a closing line, marks it.
        let head = format!("error: {}", "x".repeat(MAX_LINE_BYTES));
        let cut = format!("{head} at src/first.rs:1:1\nwork\n{head} at src/second.rs:2:2");
        let chosen = chosen_of(&cut);
        assert_eq!((chosen.lines.len(), chosen.errors), (2, 1));
        let (first, span) = &chosen.lines[0];
        assert_eq!(
            first.text.len(),
            MAX_LINE_BYTES,
            "160 bytes with the cut mark"
        );
        assert!(first.closing && cut[span.clone()].ends_with("src/first.rs:1:1"));
        assert_eq!(chosen.lines[1].0.text, "work");
        // The lines that state an error are chosen up to their bound, and
        // the closing lines are the last two of the output.
        let many = format!(
            "{}last but one\n\n\nCommand exited with code 9\n\n",
            (0..40)
                .map(|index| format!("error: number {index}\n"))
                .collect::<String>()
        );
        let (code, kept) = failed(&many);
        assert_eq!(code, Some(9));
        assert_eq!(kept.len(), MAX_ERROR_LINES + MAX_CLOSING_LINES);
        assert_eq!(
            (kept[4].as_str(), kept[5].as_str(), kept[6].as_str()),
            (
                "error: number 4",
                "last but one",
                "Command exited with code 9"
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
            // A position opens with a digit: a call and an empty pair of
            // brackets name no place, and a place after them is named.
            ("TypeError: app.use() needs a handler", None),
            ("error: main.rs(,) is no place", None),
            ("Foo.bar() failed at src/x.rs:4", Some("src/x.rs:4")),
            // The reading is by shape: a host with a port reads as a file
            // with a line.
            (
                "error: connect to redis.local:6379 refused",
                Some("redis.local:6379"),
            ),
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
            // The reading is by shape: the name of a digest reads as a
            // diagnostic code.
            ("error: SHA256 mismatch for the bundle", Some("SHA256")),
            // A name with a letter outside ASCII is read whole.
            ("ÜberError: no such file", Some("ÜberError")),
            ("raise ΣException()", Some("ΣException")),
            // A combining mark belongs to the letter it follows: a
            // decomposed accent and a virama keep the name whole.
            ("E\u{301}checError: abandon", Some("E\u{301}checError")),
            (
                "\u{915}\u{94d}\u{937}Error raised",
                Some("\u{915}\u{94d}\u{937}Error"),
            ),
            // A word with a numeric character that is no decimal digit
            // names no error, and the words after it are read.
            ("\u{bd}Error then E0308", Some("E0308")),
            ("\u{bd}Error alone", None),
            ("Parse\u{bd}Error alone", None),
            ("Parse\u{2167}Error then ValueError", Some("ValueError")),
            ("Half\u{bd}ValueError alone", None),
            ("Half\u{bd}ValueError then E0308", Some("E0308")),
            ("error: \u{345}ParseError", None),
            ("\u{93e}Exception then E0308", Some("E0308")),
            // Punctuation of any script parts a name from its line.
            ("\u{201c}ValueError\u{201d} was raised", Some("ValueError")),
            (
                "Parse\u{0663}Error: digits of any script",
                Some("Parse\u{0663}Error"),
            ),
        ] {
            assert_eq!(named_error(line), error, "{line}");
        }
        // A name of the byte limit is read, and one byte more names no
        // error and no path.
        let name = |stem: usize| format!("{}Error", "A".repeat(stem));
        let (at_bound, past) = (name(MAX_TOKEN_BYTES - 5), name(MAX_TOKEN_BYTES - 4));
        assert_eq!(named_error(&at_bound), Some(at_bound.as_str()));
        assert_eq!(named_error(&past), None);
        let path = |stem: usize| format!("{}.rs:1", "p".repeat(stem));
        let (at_bound, past) = (path(MAX_TOKEN_BYTES - 5), path(MAX_TOKEN_BYTES - 4));
        assert_eq!(named_path(&at_bound), Some(at_bound.as_str()));
        assert_eq!(named_path(&past), None);
    }

    /// The digest of a command line is stored as the lineage of its
    /// records, so its encoding is fixed: this answer was computed once
    /// from the redacted line and the two gaps asserted beside it.
    #[test]
    fn the_digest_of_a_known_command_line_is_its_stored_answer() {
        let (assignment, _) = keyed_credential();
        let second = ["s3c0nd", "-fixture", "-value-", "7731"].concat();
        let command = format!(
            "deploy --{assignment} --region eu --{}={second} --target api",
            ["access_", "tok", "en"].concat()
        );
        let redacted =
            context_core::redaction::redact_windowed_durable_text(&command, usize::MAX).unwrap();
        // The count of the command line's bytes between the finding
        // before and each finding.
        let mut end = 0;
        let gaps: Vec<usize> = redacted
            .detections
            .iter()
            .map(|finding| {
                let gap = finding.offset - end;
                end = finding.offset + finding.length;
                gap
            })
            .collect();
        assert_eq!(gaps, KNOWN_GAPS);
        assert_eq!(redacted.text, KNOWN_REDACTED);
        assert_eq!(named(&command).digest, KNOWN_DIGEST);
    }

    const KNOWN_GAPS: [usize; 2] = [19, 28];
    const KNOWN_REDACTED: &str = "deploy --api_token=<REDACTED:api_token> --region eu --access_token=<REDACTED:access_token> --target api";
    /// The first 32 hex digits of the SHA-256 of `project` and
    /// [`KNOWN_REDACTED`], each behind its length as eight little-endian
    /// bytes, and then each of [`KNOWN_GAPS`] as eight little-endian bytes.
    const KNOWN_DIGEST: &str = "5a953ad9406d1016d9768960d1a03b87";

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
        let text = "error: boom at src/app.rs:3:1\nCommand exited with code 7";
        let status_line = text.find("Command").unwrap();
        let judged = |findings: Option<Vec<Range<usize>>>| {
            let failure = screened_with(text, |_| findings);
            (failure.exit_code, failure.path, failure.excerpt)
        };
        let path = || Some("src/app.rs:3:1".to_owned());
        let one = |finding: Range<usize>| Some(std::iter::once(finding).collect::<Vec<_>>());
        // A finding that overlaps the status line, even in its first word,
        // withholds the code with the excerpt.
        assert_eq!(
            judged(one(status_line..status_line + 3)),
            (None, path(), Excerpt::Finding)
        );
        // A finding on another line withholds the excerpt and that line's
        // path, and the code stands.
        assert_eq!(judged(one(0..5)), (Some(7), None, Excerpt::Finding));
        // A scan without a finding keeps all three, and one that did not
        // finish keeps no text of the output.
        let Excerpt::Kept(lines) = judged(Some(Vec::new())).2 else {
            panic!("a clean scan keeps the lines");
        };
        assert_eq!(lines.len(), 2);
        assert_eq!(judged(Some(Vec::new())).0, Some(7));
        assert_eq!(judged(None), (None, None, Excerpt::Unscanned));
        // The shared redactor end to end: a credential on another line
        // withholds the excerpt, and the status line keeps its code.
        let (assignment, value) = keyed_credential();
        let found = recorded(
            "deploy",
            &format!("login with {assignment}\nCommand exited with code 3"),
            true,
        )
        .unwrap();
        assert!(!format!("{found:?}").contains(&value));
        assert_eq!(
            (found.exit_code, found.excerpt),
            (Some(3), Excerpt::Finding)
        );
    }

    /// A Kernel that is busy or failing, a row another writer changed, and
    /// a read past the tip leave a call for a later pass. Every other
    /// refusal would meet the same result again and settles the call.
    #[test]
    fn a_kernel_error_leaves_a_call_for_a_later_pass_or_settles_it() {
        use kernel::KernelError as E;
        let later = [
            E::Deadline,
            E::Busy,
            E::Held,
            E::Io,
            E::FenceLost,
            E::Fault,
            E::NotFound,
            E::FutureSnapshot,
            E::InvalidRestore,
        ];
        for error in E::ALL {
            assert_eq!(leaves_for_later(error), later.contains(error), "{error:?}");
        }
        assert!(!leaves_for_later(&E::AdmissionPolicy));
        assert!(!leaves_for_later(&E::InvalidInput));
    }

    /// A stored row is a record of this recorder's when its rationale
    /// states a run: a state, the session of that run, and a time the
    /// calendar renders.
    #[test]
    fn a_stored_rationale_states_its_run_or_is_no_record() {
        let failed = json!({ "state": "failed", "session": "s1", "at_ms": 1_000 });
        let stored = Stored::read(&failed.to_string()).unwrap();
        assert_eq!(
            (stored.resolved, stored.at_ms, stored.session.as_str()),
            (false, 1_000, "s1")
        );
        let resolved = json!({
            "state": "resolved", "session": "s1", "at_ms": 1_000,
            "resolved_session": "s2", "resolved_at_ms": 2_000,
        });
        let stored = Stored::read(&resolved.to_string()).unwrap();
        assert_eq!(
            (stored.resolved, stored.at_ms, stored.session.as_str()),
            (true, 2_000, "s2")
        );
        for other in [
            json!({ "session": "s1", "at_ms": 1_000 }),
            json!({ "state": "open", "session": "s1", "at_ms": 1_000 }),
            json!({ "state": "failed", "at_ms": 1_000 }),
            json!({ "state": "failed", "session": 7, "at_ms": 1_000 }),
            json!({ "state": "failed", "session": "s1" }),
            json!({ "state": "failed", "session": "s1", "at_ms": "1000" }),
            json!({ "state": "failed", "session": "s1", "at_ms": i64::MAX }),
            json!({ "state": "resolved", "session": "s1", "at_ms": 1_000 }),
            json!(["failed"]),
        ] {
            assert!(Stored::read(&other.to_string()).is_none(), "{other}");
        }
        assert!(Stored::read("no json").is_none());
    }

    /// The Pi row of an assistant message whose tool call parts are
    /// `parts`.
    fn calls_row(row: &str, parts: Vec<Value>) -> Arc<Value> {
        Arc::new(json!({ "id": row, "type": "message", "message": {
            "role": "assistant", "content": parts, "usage": {}, "stopReason": "toolUse",
            "timestamp": 1_000,
        } }))
    }

    /// A Pi tool call part that runs `make`, carrying `id` under `key`.
    fn call_part(key: &str, id: Value) -> Value {
        let mut part =
            json!({ "type": "toolCall", "name": "bash", "arguments": { "command": "make" } });
        part[key] = id;
        part
    }

    /// The Pi row of an assistant message that runs `make` under `id`.
    fn call_row(row: &str, id: &str) -> Arc<Value> {
        calls_row(row, vec![call_part("id", json!(id))])
    }

    /// The Pi row of a marked tool result that carries `id` as its
    /// `toolCallId` when `id` is given.
    fn result_row(row: &str, id: Option<Value>) -> Arc<Value> {
        let mut message = json!({
            "role": "toolResult", "toolName": "bash",
            "content": [{ "type": "text", "text": "error: boom" }], "isError": true,
            "timestamp": 1_100,
        });
        if let Some(id) = id {
            message["toolCallId"] = id;
        }
        Arc::new(json!({ "id": row, "type": "message", "message": message }))
    }

    /// The Pi row of a result that answers the call `id`.
    fn answer_row(row: &str, id: &str) -> Arc<Value> {
        result_row(row, Some(json!(id)))
    }

    /// The Pi row of a message of `role` that holds text.
    fn text_row(row: &str, role: &str) -> Arc<Value> {
        let content = match role {
            "assistant" => json!([{ "type": "text", "text": "Go on." }]),
            _ => json!("Go on."),
        };
        Arc::new(json!({ "id": row, "type": "message", "message": {
            "role": role, "content": content, "usage": {}, "stopReason": "stop",
            "timestamp": 1_000,
        } }))
    }

    /// `row` as the plugin sends a message that is another than its
    /// session entry's: its id and its message alone.
    fn untyped(row: &Arc<Value>) -> Arc<Value> {
        let mut row = Value::clone(row);
        row.as_object_mut().unwrap().remove("type");
        Arc::new(row)
    }

    /// The `(entry, call, command)` of each command result the window of
    /// `rows` pairs.
    fn paired_commands(rows: &[Arc<Value>]) -> Vec<(String, String, String)> {
        let messages = derived(rows);
        command_results(&messages, rows)
            .unwrap()
            .into_iter()
            .map(|candidate| {
                (
                    candidate.entry.to_owned(),
                    candidate.call.to_owned(),
                    candidate.command.to_owned(),
                )
            })
            .collect()
    }

    /// The window the transform handler derives from `rows`.
    fn derived(rows: &[Arc<Value>]) -> Vec<Arc<crate::wire::IngressMessage>> {
        crate::codec::pi::derive_pi_window(rows)
            .unwrap()
            .into_iter()
            .map(Arc::new)
            .collect()
    }

    /// The `(entry, call)` of each command result the window of `rows`
    /// pairs: `None` for a window that names no executions.
    fn paired(rows: &[Arc<Value>]) -> Option<Vec<(String, String)>> {
        let messages = derived(rows);
        let results = command_results(&messages, rows)?;
        Some(
            results
                .into_iter()
                .map(|candidate| (candidate.entry.to_owned(), candidate.call.to_owned()))
                .collect(),
        )
    }

    fn pairs(pairs: &[(&str, &str)]) -> Option<Vec<(String, String)>> {
        Some(
            pairs
                .iter()
                .map(|(entry, call)| ((*entry).to_owned(), (*call).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn a_result_is_paired_with_the_call_it_answers_in_its_tool_turn() {
        // Two turns that use one call id are two executions, each named by
        // the entry that holds its result.
        let reused = [
            text_row("u1", "user"),
            call_row("a1", "call_0"),
            answer_row("r1", "call_0"),
            call_row("a2", "call_0"),
            answer_row("r2", "call_0"),
        ];
        assert_eq!(
            paired(&reused),
            pairs(&[("r1", "call_0"), ("r2", "call_0")])
        );
        // An id that two calls or two results of one turn carry names
        // neither, and the turns beside it keep their pairs.
        let twice_answered = [
            call_row("a1", "c1"),
            answer_row("r1", "c1"),
            call_row("a2", "c2"),
            answer_row("r2", "c2"),
            answer_row("r3", "c2"),
            call_row("a3", "c1"),
            answer_row("r4", "c1"),
        ];
        assert_eq!(
            paired(&twice_answered),
            pairs(&[("r1", "c1"), ("r4", "c1")])
        );
        let twice_called = [
            call_row("a1", "c1"),
            answer_row("r1", "c1"),
            calls_row(
                "a2",
                vec![call_part("id", json!("c1")), call_part("id", json!("c1"))],
            ),
            answer_row("r2", "c1"),
        ];
        assert_eq!(paired(&twice_called), pairs(&[("r1", "c1")]));
        // A result after its call's turn has closed answers no call: a
        // user message, or the assistant's next message, stands between.
        for between in ["user", "assistant"] {
            let late = [
                call_row("a1", "c1"),
                text_row("m1", between),
                answer_row("r1", "c1"),
            ];
            assert_eq!(paired(&late), pairs(&[]), "{between}");
        }
        // A result after the turn's close shares no id with the turn: the
        // turn's pair stands, and the late result answers no call.
        let after_close = [
            call_row("a1", "c1"),
            answer_row("r1", "c1"),
            text_row("u1", "user"),
            answer_row("r2", "c1"),
        ];
        assert_eq!(paired(&after_close), pairs(&[("r1", "c1")]));
        // A call that carries no command line pairs with no result.
        let read = [
            calls_row(
                "a1",
                vec![json!({
                    "type": "toolCall", "id": "c1", "name": "read",
                    "arguments": { "path": "src/lib.rs" },
                })],
            ),
            answer_row("r1", "c1"),
        ];
        assert_eq!(paired(&read), pairs(&[]));
    }

    /// A closed turn holds empty tables, so the turn after a wide one grows
    /// its own.
    #[test]
    fn a_closed_turn_keeps_no_table_of_a_wide_turn() {
        let ids: Vec<String> = (0..4096).map(|n| format!("call-{n}")).collect();
        let mut turn = Turn::default();
        for id in &ids {
            turn.named.insert(id.as_str());
            turn.answered.insert(id.as_str());
            turn.shared.insert(id.as_str());
        }
        let mut kept = VecDeque::new();
        turn.close(&mut kept);
        assert_eq!(
            (
                turn.calls.capacity(),
                turn.named.capacity(),
                turn.answered.capacity(),
                turn.shared.capacity(),
                turn.paired.capacity(),
            ),
            (0, 0, 0, 0, 0)
        );
        assert!(kept.is_empty());
    }

    /// The window keeps its last 1,024 pairs. A pair under an id its turn
    /// carries twice takes no place among them.
    #[test]
    fn a_pair_under_a_shared_id_takes_no_place_among_the_pairs_kept() {
        let mut rows = Vec::new();
        for index in 0..MEMO_CALLS_PER_SESSION {
            rows.push(call_row(&format!("a{index}"), "c1"));
            rows.push(answer_row(&format!("r{index}"), "c1"));
        }
        rows.push(call_row("a-twice", "c2"));
        rows.push(answer_row("r-twice-1", "c2"));
        rows.push(answer_row("r-twice-2", "c2"));
        let kept = paired(&rows).unwrap();
        assert_eq!(kept.len(), MEMO_CALLS_PER_SESSION);
        assert_eq!(kept[0], ("r0".to_owned(), "c1".to_owned()));
        // One pair more leaves the oldest out.
        rows.push(call_row("a-last", "c3"));
        rows.push(answer_row("r-last", "c3"));
        let kept = paired(&rows).unwrap();
        assert_eq!(kept.len(), MEMO_CALLS_PER_SESSION);
        assert_eq!(kept[0], ("r1".to_owned(), "c1".to_owned()));
    }

    /// A call counts when the tool call part at its place in its row
    /// carries the call's id itself, and a result when its row carries its
    /// `toolCallId` itself. The Pi codec gives a result row without a
    /// `toolCallId` the id `tool` and a call part without an id a
    /// synthesized one; neither names an execution.
    #[test]
    fn an_id_the_native_row_does_not_carry_pairs_no_result() {
        // A call whose id is the word the codec gives an id-less result:
        // the result that carries the id is its answer.
        let named = [call_row("0000000a", "tool"), answer_row("0000000b", "tool")];
        assert_eq!(paired(&named), pairs(&[("0000000b", "tool")]));
        // A result row without a `toolCallId`, or with one that is no
        // string or is blank, answers no call. The codec reads the first
        // two as `tool`, the id of the call beside them.
        for id in [None, Some(json!(7)), Some(json!("")), Some(json!(" "))] {
            let rows = [
                call_row("0000000a", "tool"),
                result_row("0000000b", id.clone()),
            ];
            assert_eq!(paired(&rows), pairs(&[]), "{id:?}");
        }
        // A blank id that the call part and the result row both carry
        // names no execution.
        let blank = [call_row("0000000a", " "), answer_row("0000000b", " ")];
        let decoded = derived(&blank);
        let BlockKind::ToolCall { id, .. } = decoded[0].ck.content()[0].kind() else {
            panic!("a tool call block");
        };
        assert_eq!(id, " ", "the codec keeps the blank id on the block");
        assert_eq!(paired(&blank), pairs(&[]));
        // A call part that carries its id as `callId` pairs under it, and
        // an `id` that is a string comes first.
        let by_call_id = [
            calls_row("0000000a", vec![call_part("callId", json!("c9"))]),
            answer_row("0000000b", "c9"),
        ];
        assert_eq!(paired(&by_call_id), pairs(&[("0000000b", "c9")]));
        let mut both = call_part("id", json!("c1"));
        both["callId"] = json!("c9");
        for (answered, pair) in [("c1", pairs(&[("0000000b", "c1")])), ("c9", pairs(&[]))] {
            let rows = [
                calls_row("0000000a", vec![both.clone()]),
                answer_row("0000000b", answered),
            ];
            assert_eq!(paired(&rows), pair, "{answered}");
        }
        // A call part without an id decodes to a synthesized one. A result
        // row that carries that id answers no call, also when another part
        // of the call's row carries the id as its own.
        let bare =
            json!({ "type": "toolCall", "name": "bash", "arguments": { "command": "make" } });
        let synthesized = derived(&[calls_row("0000000a", vec![bare.clone()])]);
        let BlockKind::ToolCall { id, .. } = synthesized[0].ck.content()[0].kind() else {
            panic!("a tool call block");
        };
        let id = &id.clone();
        let alone = [
            calls_row("0000000a", vec![bare.clone()]),
            answer_row("0000000b", id),
        ];
        assert_eq!(paired(&alone), pairs(&[]));
        let lent = json!({
            "type": "toolCall", "id": id, "name": "read", "arguments": { "path": "src/lib.rs" },
        });
        let beside = [
            calls_row("0000000a", vec![bare.clone(), lent]),
            answer_row("0000000b", id),
        ];
        assert_eq!(paired(&beside), pairs(&[]));
        // A text part that holds the id lends it to no call.
        let text_lends = [
            calls_row(
                "0000000a",
                vec![
                    bare.clone(),
                    json!({ "type": "text", "id": id, "text": "Running." }),
                ],
            ),
            answer_row("0000000b", id),
        ];
        assert_eq!(paired(&text_lends), pairs(&[]));
        // A command call that carries that id itself, after the call
        // without one, is the call the result answers.
        let own = json!({
            "type": "toolCall", "id": id, "name": "bash",
            "arguments": { "command": "make check" },
        });
        let owned = [
            calls_row("0000000a", vec![bare, own]),
            answer_row("0000000b", id),
        ];
        assert_eq!(
            paired_commands(&owned),
            [("0000000b".to_owned(), id.clone(), "make check".to_owned())]
        );
        // A call takes the place of its part among the row's tool call
        // parts: a text part ahead of it takes none, and two calls of one
        // row are each answered by their own result.
        let check = |id: &str, command: &str| {
            json!({
                "type": "toolCall", "id": id, "name": "bash", "arguments": { "command": command },
            })
        };
        let two = [
            calls_row(
                "0000000a",
                vec![
                    json!({ "type": "text", "text": "Running both." }),
                    check("c1", "make lint"),
                    json!({ "type": "text", "text": "And." }),
                    check("c2", "make test"),
                ],
            ),
            answer_row("0000000b", "c2"),
            answer_row("0000000c", "c1"),
        ];
        assert_eq!(
            paired_commands(&two),
            [
                (
                    "0000000b".to_owned(),
                    "c2".to_owned(),
                    "make test".to_owned()
                ),
                (
                    "0000000c".to_owned(),
                    "c1".to_owned(),
                    "make lint".to_owned()
                ),
            ]
        );
        // A composite id pairs by its whole native text.
        let composite = [
            call_row("0000000a", "call-1|item-1"),
            answer_row("0000000b", "call-1|item-1"),
            call_row("0000000c", "call-1|item-2"),
            answer_row("0000000d", "call-1|item-2"),
        ];
        assert_eq!(
            paired(&composite),
            pairs(&[("0000000b", "call-1|item-1"), ("0000000d", "call-1|item-2")])
        );
    }

    /// A window names executions when each of its messages is the message
    /// of the row at its place and no two rows share an id.
    #[test]
    fn a_window_whose_rows_are_not_its_messages_rows_names_no_execution() {
        let rows = [call_row("a1", "c1"), answer_row("r1", "c1")];
        let messages = derived(&rows);
        assert!(command_results(&messages, &rows).is_some());
        // Two turns whose result rows share one entry id and one call id
        // would be one execution.
        let repeated = [
            call_row("a1", "call_0"),
            answer_row("r1", "call_0"),
            call_row("a2", "call_0"),
            answer_row("r1", "call_0"),
        ];
        assert_eq!(paired(&repeated), None);
        // Rows of another count, and rows in another order.
        assert!(command_results(&messages, &rows[..1]).is_none());
        assert!(command_results(&messages, &[]).is_none());
        let swapped = [rows[1].clone(), rows[0].clone()];
        assert!(command_results(&messages, &swapped).is_none());
        // An entry id of the transform's bound names an execution, and one
        // byte more names none.
        for (bytes, named) in [
            (crate::wire::MAX_MID_BYTES, true),
            (crate::wire::MAX_MID_BYTES + 1, false),
        ] {
            let entry = "r".repeat(bytes);
            let long = [call_row("a1", "c1"), answer_row(&entry, "c1")];
            assert_eq!(paired(&long).is_some(), named, "{bytes}");
        }
    }

    /// A row carries its session entry's `type` when its message is the
    /// entry's message. A call or a result whose row carries none counts
    /// for nothing: the call is answered by no result, and the result
    /// changes no record.
    #[test]
    fn a_row_that_is_no_message_entry_of_the_session_names_no_execution() {
        let rows = [call_row("a1", "c1"), answer_row("r1", "c1")];
        assert_eq!(paired(&rows), pairs(&[("r1", "c1")]));
        let edited_result = [rows[0].clone(), untyped(&rows[1])];
        assert_eq!(paired(&edited_result), pairs(&[]));
        let edited_call = [untyped(&rows[0]), rows[1].clone()];
        assert_eq!(paired(&edited_call), pairs(&[]));
        // A type of another entry kind, and one that is no string.
        for kind in [json!("custom_message"), json!(null), json!(7)] {
            let mut other = Value::clone(&rows[1]);
            other["type"] = kind.clone();
            assert_eq!(
                paired(&[rows[0].clone(), Arc::new(other)]),
                pairs(&[]),
                "{kind}"
            );
        }
        // A second result under the id in an untyped row shares no id with
        // the turn: the typed pair stands.
        let beside = [
            rows[0].clone(),
            rows[1].clone(),
            untyped(&answer_row("r2", "c1")),
        ];
        assert_eq!(paired(&beside), pairs(&[("r1", "c1")]));
    }

    /// The memo's judged calls stand in one Kernel history. Under another
    /// history they are given up, and the sessions' entries stay.
    #[test]
    fn the_memo_gives_up_its_judged_calls_under_another_kernel_history() {
        let directory = tempfile::tempdir().unwrap();
        let kernel = kernel::KernelStore::open(directory.path()).unwrap();
        let history = kernel.capture_commit_read_target().unwrap().incarnation;
        let mut memo = FailureMemo::default();
        let epoch = memo.pin("s1");
        memo.follow(history);
        assert!(memo.learn("s1", epoch, 7, history));
        memo.follow(history);
        assert!(memo.has_seen("s1", 7), "the same history keeps the call");
        let budget = EvalBudget::new(None, Arc::new(AtomicBool::new(false)));
        assert_eq!(history_of(&kernel, Some(history), &budget), Ok(history));
        assert_eq!(history_of(&kernel, None, &budget), Ok(history));
        // A restore installs another history.
        let backups = tempfile::tempdir().unwrap();
        std::fs::set_permissions(
            backups.path(),
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
        )
        .unwrap();
        let backup = kernel
            .backup(kernel::BackupRequest {
                destination_directory: backups.path().to_path_buf(),
                deadline: Instant::now() + std::time::Duration::from_secs(30),
                capture_pin_expires_at: None,
            })
            .unwrap();
        kernel.restore(&backup.destination_path).unwrap();
        let restored = history_of(&kernel, Some(history), &budget).unwrap();
        assert_ne!(restored, history);
        memo.follow(restored);
        assert!(!memo.has_seen("s1", 7));
        assert!(memo.current("s1", epoch) && memo.busy("s1"));
        // A call judged under the replaced history is kept out, and one
        // judged under the history the memo stands in is learned.
        assert!(!memo.learn("s1", epoch, 7, history));
        assert!(!memo.has_seen("s1", 7));
        assert!(memo.learn("s1", epoch, 7, restored));
        assert!(memo.has_seen("s1", 7));
        // A pass reads the memo's history while the Kernel holds it, and
        // gives the judged calls up once the Kernel holds another.
        assert_eq!(memo.history_in(&kernel), Some(restored));
        assert!(memo.has_seen("s1", 7));
        kernel.restore(&backup.destination_path).unwrap();
        assert_eq!(memo.history_in(&kernel), None);
        assert!(!memo.has_seen("s1", 7));
        assert!(memo.current("s1", epoch) && memo.busy("s1"));
        assert_eq!(
            memo.history_in(&kernel),
            None,
            "a memo that holds no history"
        );
        // A memo without a history learns no call.
        assert!(!memo.learn("s1", epoch, 7, restored));
        assert!(!memo.has_seen("s1", 7));
    }

    /// The writer's commit judges the composed record itself and requires
    /// a record id that is new to the Kernel, on a new lineage and in place
    /// of a live record alike.
    #[test]
    fn a_commit_refuses_a_record_the_redactor_would_change_and_an_id_another_row_holds() {
        let (directory, checkout) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let kernel = kernel::KernelStore::open(directory.path()).unwrap();
        let project = ProjectBinding::new(checkout.path());
        let writer = Writer {
            kernel: &kernel,
            project: &project,
            scope: project.digest(),
            session: "s1",
            before_commit: None,
        };
        let history = kernel.capture_commit_read_target().unwrap().incarnation;
        let budget = EvalBudget::new(None, Arc::new(AtomicBool::new(false)));
        let lineage = format!("{FAILURE_SOURCE_ID}:{}", named("make").digest);
        let clean = failed_record(
            "s1",
            &observed("make"),
            &failure(Some(1), Excerpt::Kept(Vec::new())),
        );
        let commit = |call: &str, revision, target: Option<&str>, payload| {
            let id = record_id(writer.scope, "s1", "entry-1", call);
            let row = Row {
                key: writer.record_key("s1", &id).unwrap(),
                id: id.clone(),
                lineage: &lineage,
                revision,
                target,
                payload,
            };
            (id, writer.commit(row, history, &budget).unwrap())
        };
        let live = || {
            let (_, newest) = kernel
                .newest_decisions_on_lineage_within_budget(&budget, CAPTURE_SOURCE_KIND, &lineage)
                .unwrap();
            newest
                .iter()
                .filter(|row| row.invalidated_commit_seq.is_none())
                .map(|row| row.object_id.clone())
                .collect::<Vec<_>>()
        };
        let (credential, _) = keyed_credential();
        let with = |summary: bool| {
            let mut payload = clean.clone();
            match summary {
                true => payload.summary.push_str(&format!("\n> {credential}")),
                false => payload.rationale = json!({ "note": credential }).to_string(),
            }
            payload
        };
        let changed = Judged::Left("the redactor changes the record's text");
        // On a new lineage.
        for (call, summary) in [("call-a", true), ("call-b", false)] {
            let tip = kernel.tip().unwrap();
            assert_eq!(commit(call, 1, None, with(summary)).1, changed, "{call}");
            assert_eq!(kernel.tip().unwrap(), tip, "{call}");
        }
        let (first, wrote) = commit("call-1", 1, None, clean.clone());
        assert_eq!(wrote, Judged::Wrote);
        assert_eq!(live(), std::slice::from_ref(&first));
        // In place of the live record, which stays live.
        for (call, summary) in [("call-c", true), ("call-d", false)] {
            let tip = kernel.tip().unwrap();
            let judged = commit(call, 2, Some(&first), with(summary)).1;
            assert_eq!(judged, changed, "{call}");
            assert_eq!(kernel.tip().unwrap(), tip, "{call}");
            assert_eq!(live(), std::slice::from_ref(&first), "{call}");
        }
        // A row that another commit wrote under the record's id stays as
        // it is: this commit writes nothing in its place or beside it.
        let occupied = Judged::Left("another row holds the record's id");
        let foreign = |key: &str, target: Option<&str>| {
            let tip = kernel.tip().unwrap();
            let row = Row {
                id: first.clone(),
                key: writer.record_key("s1", key).unwrap(),
                lineage: &lineage,
                revision: 2,
                target,
                payload: clean.clone(),
            };
            assert_eq!(writer.commit(row, history, &budget).unwrap(), occupied);
            assert_eq!(kernel.tip().unwrap(), tip);
        };
        foreign("another-key", None);
        let (second, wrote) = commit("call-2", 2, Some(&first), clean.clone());
        assert_eq!(wrote, Judged::Wrote);
        assert_eq!(live(), std::slice::from_ref(&second));
        foreign("a-third-key", Some(&second));
        assert_eq!(live(), std::slice::from_ref(&second));
        // A live row at a later revision of the lineage holds the id the
        // next record plans, while the record it would take the place of
        // is live: both rows stay live and the commit logs no receipt.
        let (third, wrote) = commit("call-3", 5, None, clean.clone());
        assert_eq!(wrote, Judged::Wrote);
        assert_eq!(live(), [third.clone(), second.clone()]);
        let tip = kernel.tip().unwrap();
        let key = writer.record_key("s1", "a-fourth-key").unwrap();
        let row = Row {
            id: third.clone(),
            key: key.clone(),
            lineage: &lineage,
            revision: 3,
            target: Some(&second),
            payload: clean.clone(),
        };
        assert_eq!(writer.commit(row, history, &budget).unwrap(), occupied);
        assert_eq!(kernel.tip().unwrap(), tip);
        assert!(!writer.committed(&key, &budget).unwrap());
        assert_eq!(live(), [third, second]);
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
        assert_eq!(
            (
                MAX_ERROR_LINES,
                MAX_CLOSING_LINES,
                MAX_LINE_BYTES,
                MAX_SUMMARY_BYTES
            ),
            (5, 2, 160, 900)
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
        // not render, is read for no judgment.
        let output = OutputKind::ErrorText {
            text: "error: boom".into(),
        };
        let candidate = |entry, call_id, tool, at_ms| Candidate {
            entry,
            call: call_id,
            tool,
            command: "make",
            output: &output,
            at_ms,
        };
        let of = |at_ms| candidate("entry-1", "call-1", "bash", at_ms).readable();
        assert!(of(Some(1_000)).is_some());
        // An id or a tool name that is empty or past 1,024 bytes is left
        // out before a byte of it is copied.
        let (at_bound, past) = ("i".repeat(1_024), "i".repeat(1_025));
        let timed = Some(1_000);
        assert!(
            candidate(&at_bound, &at_bound, &at_bound, timed)
                .readable()
                .is_some()
        );
        for (entry, call_id, tool) in [
            (past.as_str(), "call-1", "bash"),
            ("entry-1", past.as_str(), "bash"),
            ("entry-1", "call-1", past.as_str()),
            ("", "call-1", "bash"),
            ("entry-1", "", "bash"),
            ("entry-1", "call-1", ""),
        ] {
            assert!(candidate(entry, call_id, tool, timed).readable().is_none());
        }
        for at_ms in [
            None,
            Some(i64::MAX),
            Some(8_700_000_000_000_000),
            Some(i64::MIN),
        ] {
            assert!(of(at_ms).is_none(), "{at_ms:?}");
        }
    }

    /// A pass charges a result the bytes its copy holds: the text of a
    /// marked result counts, and an unmarked result's text is left in the
    /// window and counts nothing.
    #[test]
    fn a_result_is_charged_the_bytes_its_copy_holds() {
        let call = "cargo test -p api";
        let text = "error: boom\n\nCommand exited with code 1";
        for (output, marked) in [
            (OutputKind::ErrorText { text: text.into() }, Some(text)),
            (OutputKind::Text { text: text.into() }, None),
        ] {
            let candidate = Candidate {
                entry: "entry-1",
                call: "call-1",
                tool: "bash",
                command: call,
                output: &output,
                at_ms: Some(1_000),
            };
            let readable = candidate.readable().unwrap();
            let copy = readable.copied();
            assert_eq!(copy.marked.as_deref(), marked);
            assert_eq!(
                readable.bytes(),
                copy.entry.len()
                    + copy.call.len()
                    + copy.tool.len()
                    + copy.command.len()
                    + copy.marked.map_or(0, |text| text.len())
            );
            let charged = "entry-1call-1bashcargo test -p api".len() + marked.map_or(0, str::len);
            assert_eq!(readable.bytes(), charged);
        }
        // A result that is not read holds no bytes: a command line and a
        // text past their bound, and a value.
        let at_bound = "x".repeat(MAX_OUTPUT_BYTES);
        let long = format!("{at_bound}x");
        let value = OutputKind::Json { value: json!(1) };
        let boom = OutputKind::ErrorText {
            text: "boom".into(),
        };
        for (command, output, read) in [
            (at_bound.as_str(), &boom, true),
            (long.as_str(), &boom, false),
            (call, &value, false),
        ] {
            let candidate = Candidate {
                entry: "entry-1",
                call: "call-1",
                tool: "bash",
                command,
                output,
                at_ms: Some(1_000),
            };
            assert_eq!(candidate.readable().is_some(), read);
        }
    }

    fn observed(command: &str) -> Observed {
        Observed {
            entry: "entry-1".into(),
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
        let tail = ".\nCommand: cargo test [arguments left out]\nPath named: src/render.rs:9:5\nError named: E0308\nOutput lines:\n> error: first\n> the end";
        let unit = observed("cargo test -p api");
        for (code, stated) in [
            (
                Some(101),
                "the harness marked the result as an error and its last line states exit code 101",
            ),
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
        // A call an extension blocked is a marked result too, and its
        // reason can end in the words of an exit line. The record states
        // what the result's last line states and claims no exit of the
        // command, for this result and for a command that did exit.
        let blocked = "deploys wait for a review\nCommand exited with code 7";
        let exited = "error: no route to host\n\nCommand exited with code 7";
        for text in [blocked, exited] {
            let failure = recorded("deploy", text, true).unwrap();
            assert_eq!(failure.exit_code, Some(7));
            let summary = summary_of(&observed("deploy"), "s1", &failure);
            let first = summary.lines().next().unwrap();
            assert!(
                first.ends_with(
                    ": the harness marked the result as an error and its last line states exit code 7."
                ),
                "{first}"
            );
            assert!(!summary.contains("the command exited"), "{summary}");
        }
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
        for (excerpt, reason) in [
            (
                Excerpt::Finding,
                "the redactor found credential-shaped content in the output",
            ),
            (
                Excerpt::Unscanned,
                "the redactor's scan of the output did not finish",
            ),
            (
                Excerpt::Refusal,
                "the output begins with the heading of the harness's report of a refused call",
            ),
        ] {
            let withheld = failed_record(&session, &unit, &at_limits(excerpt));
            assert!(
                withheld.summary.contains(&format!(
                    "\nOutput text withheld: {reason}. The text stands"
                )),
                "{}",
                withheld.summary
            );
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
    /// The fixture rests on that reading of an empty assignment by the
    /// redactor; the writer's test pins the refusal with a credential.
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
        assert_eq!(record_id("p", "s", "e", "c").len(), 36);
        assert_eq!(record_id("p", "s", "e", "c"), record_id("p", "s", "e", "c"));
        assert_ne!(record_id("ps", "", "e", "c"), record_id("p", "s", "e", "c"));
        assert_ne!(record_id("p", "se", "", "c"), record_id("p", "s", "e", "c"));
        assert_ne!(record_id("p", "s", "ec", ""), record_id("p", "s", "e", "c"));
        // One call id under another entry names another execution.
        assert_ne!(record_id("p", "s", "f", "c"), record_id("p", "s", "e", "c"));
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
        // The stored encodings, computed once outside this module: the
        // SHA-256 of `tool_failure` and the four parts, each behind its
        // length as eight little-endian bytes, and the first eight bytes
        // of the session id's SHA-256 as a big-endian number.
        assert_eq!(
            record_id("project-digest", "session-1", "entry-0007", "call_9|item"),
            "mem_0baafb97e025e9429e0a6759e0a5e35c"
        );
        assert_eq!(
            session_keys("session-1"),
            (
                "9514267805222021320:".to_owned(),
                "9514267805222021320;".to_owned()
            )
        );
        let (from, to) = session_keys("s1");
        assert!(from < to && from.ends_with(':') && to.ends_with(';'));
        assert_ne!(session_keys("s2").0, from);
        // A record's id is `mem_` and 32 lowercase hexadecimal digits.
        let id = record_id("p", "s", "e", "c");
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
        let (_directory, history) = a_history();
        let memo = Mutex::new(FailureMemo::default());
        memo.lock().unwrap().follow(history);
        let gate = Mutex::new(());
        let pin = || memo.lock().unwrap().pin("s1");
        let current = |epoch| memo.lock().unwrap().current("s1", epoch);
        let before = pin();
        assert!(memo.lock().unwrap().learn("s1", before, 7, history));
        assert!(memo.lock().unwrap().has_seen("s1", 7));
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

    /// The history of a Kernel of its own, with the directory that holds
    /// the Kernel.
    fn a_history() -> (tempfile::TempDir, kernel::CommitReadIncarnation) {
        let directory = tempfile::tempdir().unwrap();
        let kernel = kernel::KernelStore::open(directory.path()).unwrap();
        let history = kernel.capture_commit_read_target().unwrap().incarnation;
        (directory, history)
    }

    #[test]
    fn the_memo_holds_its_bound_of_calls_sessions_and_checkpoints() {
        let (_directory, history) = a_history();
        let mut memo = FailureMemo::default();
        memo.follow(history);
        let epoch = memo.pin("s1");
        assert!(memo.busy("s1"), "one checkpoint of a session at a time");
        assert!(!memo.busy("s2"));
        for call in 0..MEMO_CALLS_PER_SESSION as u64 + 1 {
            assert!(memo.learn("s1", epoch, call, history));
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
        assert!(!memo.learn("s1", epoch, 7, history));
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

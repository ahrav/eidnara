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
/// The first line of a result states an exit code when it ends within these
/// bytes of the output.
const MAX_FIRST_LINE_BYTES: usize = 1024;
/// Closing lines read for a stated exit code.
const EXIT_CODE_CLOSING_LINES: usize = 3;
/// The commands of one command line that are read, and the words of all of
/// them. A command line with more of either is outside the reader's grammar
/// (see [`commands_of`]), which bounds the reader's tables.
const MAX_STEPS: usize = 64;
const MAX_WORDS: usize = 1024;
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
/// command line and the reader's tables of commands and words, which
/// [`MAX_STEPS`] and [`MAX_WORDS`] bound below one copy. Reading the output
/// holds the table of its lines, one entry of 32 bytes for each line and so
/// at most 32 bytes for each byte of the output and one entry more, the
/// echo's text, its copy with whitespace runs as single spaces, its two
/// parts, and such a copy of the output's last line. Judging the output
/// holds the redactor's copies of it. Eight copies of a text of
/// [`MAX_OUTPUT_BYTES`] cover the copies of each of these phases, which
/// follow one another.
const CHECKPOINT_SCRATCH_BYTES: usize = (32 + 8) * MAX_OUTPUT_BYTES;
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

/// What makes a result a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Evidence {
    /// The harness marked the result as an error.
    Marked,
    /// The command line's final `echo` printed its exit status, and it is
    /// nonzero.
    Printed,
    /// A status line of the unmarked output states a nonzero exit code.
    Stated,
}

impl Evidence {
    /// The name of the ground in a record's rationale.
    const fn name(self) -> &'static str {
        match self {
            Self::Marked => "tool_status",
            Self::Printed => "printed_status",
            Self::Stated => "stated_status",
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

/// What a result states of its command's outcome, before the redactor has
/// judged the output (see [`outcome_of`], [`screened`]).
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    /// The harness marked the result as an error.
    flagged: bool,
    /// The exit code the deciding status line states, zero included, with
    /// the bytes of that line: the harness's line for a marked result, and
    /// a tool's or a runner's for an unmarked result of a command line
    /// without a final echo of its status.
    stated: Option<(i64, Range<usize>)>,
    /// The command line reports its exit status by a final `echo` (see
    /// [`status_echo`]).
    echoes: bool,
    /// The exit status that echo printed in the output's last non-empty
    /// line, with the bytes of that line.
    printed: Option<(i64, Range<usize>)>,
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
/// [`MAX_OUTPUT_BYTES`], which source capture retains no span of. The
/// length is read before a byte is copied.
fn text_of(output: &OutputKind) -> Option<(String, bool)> {
    fn single(blocks: &[memory_store::ResultBlock]) -> Option<&String> {
        let mut texts = blocks.iter().filter_map(|block| match &block.kind {
            ResultBlockKind::Text { text } => Some(text),
            _ => None,
        });
        let text = texts.next()?;
        texts.next().is_none().then_some(text)
    }
    let whole =
        |text: &String| (text.len() <= MAX_OUTPUT_BYTES && !text.is_empty()).then(|| text.clone());
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

/// How a command of a command line is joined to the command after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Join {
    /// `&&`.
    And,
    /// `||`.
    Or,
    /// `|`.
    Pipe,
    /// `;` or a line break: the next command runs whatever this one
    /// reported.
    Then,
}

/// One command of a command line.
#[derive(Debug, Default, PartialEq, Eq)]
struct Step<'a> {
    /// The words as the command line writes them, quotes included.
    words: Vec<&'a str>,
    /// The operator that joins the command to the next one.
    join: Option<Join>,
    /// The status parameters (see [`status_parameter`]) the shell expands
    /// in the words.
    statuses: usize,
    /// The command holds a redirection.
    redirects: bool,
}

/// The first words that open a compound command or change what a command's
/// status means. A command line with one in a command's first place is
/// outside the reader's grammar.
const RESERVED_WORDS: [&str; 17] = [
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "select", "function", "coproc", "!", "[[",
];

/// The commands read so far and the word and the command under way.
#[derive(Default)]
struct LineReader<'a> {
    steps: Vec<Step<'a>>,
    step: Step<'a>,
    /// Where the word under way starts.
    word: Option<usize>,
    words: usize,
}

impl<'a> LineReader<'a> {
    /// Ends the word under way at `end`. `None` past [`MAX_WORDS`].
    fn end_word(&mut self, command: &'a str, end: usize) -> Option<()> {
        if let Some(start) = self.word.take() {
            self.words += 1;
            if self.words > MAX_WORDS {
                return None;
            }
            self.step.words.push(&command[start..end]);
        }
        Some(())
    }

    /// Ends the command under way with the operator `join`. A line break
    /// with no command before it is a blank line or follows an operator and
    /// joins nothing, and a `;` or a line break that ends the command line
    /// joins nothing. `None` for an operator with no command before it, for
    /// one that ends the command line, and past [`MAX_STEPS`].
    fn end_step(&mut self, join: Option<Join>, line_break: bool) -> Option<()> {
        if self.step.words.is_empty() {
            return match (join, self.steps.last_mut()) {
                (Some(Join::Then), _) if line_break => Some(()),
                (None, Some(last)) if last.join == Some(Join::Then) => {
                    last.join = None;
                    Some(())
                }
                _ => None,
            };
        }
        if self.steps.len() == MAX_STEPS {
            return None;
        }
        let mut step = std::mem::take(&mut self.step);
        step.join = join;
        self.steps.push(step);
        Some(())
    }
}

/// The length of the shell parameter that expands to an exit status at the
/// start of `text`: `$?` and `${?}`, the status of the last command, and
/// `$PIPESTATUS`, `${PIPESTATUS}`, and `${PIPESTATUS[<index>]}`, the
/// statuses of the commands of the last pipeline. A longer name that
/// begins with `PIPESTATUS` is another parameter.
fn status_parameter(text: &str) -> Option<usize> {
    const NAME: &str = "PIPESTATUS";
    if text.starts_with("$?") {
        return Some(2);
    }
    if text.starts_with("${?}") {
        return Some(4);
    }
    if let Some(after) = text
        .strip_prefix('$')
        .and_then(|rest| rest.strip_prefix(NAME))
    {
        let named = after
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        return (!named).then_some(1 + NAME.len());
    }
    let braced = text.strip_prefix("${")?.strip_prefix(NAME)?;
    if braced.starts_with('}') {
        return Some(NAME.len() + 3);
    }
    let index = braced.strip_prefix('[')?;
    let plain = index
        .bytes()
        .take_while(|byte| byte.is_ascii_digit() || matches!(byte, b'@' | b'*'))
        .count();
    (plain > 0 && index[plain..].starts_with("]}")).then_some(NAME.len() + plain + 5)
}

/// The bytes of the expansion that `text` opens with its first character,
/// a `$`, and whether it is a [`status_parameter`]: `$$`, a braced
/// parameter that is a name alone, or the `$` itself, which stands as
/// written before a name, another special parameter, or any other
/// character. `None` for a command substitution, for arithmetic in `$[…]`,
/// and for a braced parameter that holds more than a name, which are
/// outside the reader's grammar.
fn expansion(text: &str) -> Option<(usize, bool)> {
    if let Some(length) = status_parameter(text) {
        return Some((length, true));
    }
    // `$$` is the shell's process id, so a `?` after it is text.
    if text.starts_with("$$") {
        return Some((2, false));
    }
    let Some(braced) = text.strip_prefix("${") else {
        return (!text.starts_with("$(") && !text.starts_with("$[")).then_some((1, false));
    };
    let name = braced
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        .count();
    (name > 0 && braced[name..].starts_with('}')).then_some((name + 3, false))
}

/// The commands of a command line, in the one grammar this reader follows
/// whole, or `None` for a command line that holds anything outside it. The
/// grammar is the one section 7.9 of the host wire protocol states: words
/// that spaces and tabs part, single and double quotes, backslash escapes,
/// a backslash and line break between two words, redirections, comments,
/// the `$` forms that [`expansion`] reads, and the operators `&&`, `||`,
/// `|`, `;`, and a line break between commands, within [`MAX_STEPS`]
/// commands and [`MAX_WORDS`] words. One of [`RESERVED_WORDS`] in a
/// command's first place, an operator with no command on one of its sides,
/// and a command line with no command are outside it. The reader finds a
/// final status echo and the words a record shows; the scan passes over
/// each byte once.
fn commands_of(command: &str) -> Option<Vec<Step<'_>>> {
    let bytes = command.as_bytes();
    let mut reader = LineReader::default();
    let mut quote: Option<char> = None;
    // Where the `<` or `>` of a redirection read last ends.
    let mut redirected = usize::MAX;
    let mut skip = 0;
    for (index, ch) in command.char_indices() {
        if index < skip {
            continue;
        }
        let after = index + ch.len_utf8();
        let next = bytes.get(after).copied();
        if let Some(open) = quote {
            match ch {
                _ if ch == open => quote = None,
                '\\' if open == '"' => {
                    let escaped = command[after..].chars().next()?;
                    if escaped == '\n' {
                        return None;
                    }
                    skip = after + escaped.len_utf8();
                }
                '`' if open == '"' => return None,
                '$' if open == '"' => {
                    let (length, status) = expansion(&command[index..])?;
                    reader.step.statuses += usize::from(status);
                    skip = index + length;
                }
                _ => {}
            }
            continue;
        }
        match ch {
            ' ' | '\t' => reader.end_word(command, index)?,
            '\n' | ';' => {
                if ch == ';' && next == Some(b';') {
                    return None;
                }
                reader.end_word(command, index)?;
                reader.end_step(Some(Join::Then), ch == '\n')?;
            }
            '|' => {
                if next == Some(b'&') || redirected == index {
                    return None;
                }
                reader.end_word(command, index)?;
                let join = match next {
                    Some(b'|') => {
                        skip = after + 1;
                        Join::Or
                    }
                    _ => Join::Pipe,
                };
                reader.end_step(Some(join), false)?;
            }
            '&' if next == Some(b'&') => {
                reader.end_word(command, index)?;
                skip = after + 1;
                reader.end_step(Some(Join::And), false)?;
            }
            '&' => {
                // The `&` of `>&2`, `2>&1`, `<&3`, and `&>file` is part of
                // a redirection, which is a word of its command.
                if !(next == Some(b'>') || redirected == index) {
                    return None;
                }
                reader.step.redirects = true;
                reader.word.get_or_insert(index);
            }
            '<' | '>' => {
                if ch == '<' && next == Some(b'<') {
                    return None;
                }
                redirected = after;
                reader.step.redirects = true;
                reader.word.get_or_insert(index);
            }
            '(' | ')' | '{' | '}' | '`' | '\r' => return None,
            // A `#` that begins a word opens a comment, which runs to the
            // end of its line.
            '#' if reader.word.is_none() => {
                skip = command[index..]
                    .find('\n')
                    .map_or(command.len(), |end| index + end);
            }
            '\\' => {
                let escaped = command[after..].chars().next()?;
                skip = after + escaped.len_utf8();
                // The shell removes a backslash and the line break after
                // it, which between two words leaves the words as they are.
                match (escaped, reader.word) {
                    ('\n', Some(_)) => return None,
                    ('\n', None) => {}
                    _ => {
                        reader.word.get_or_insert(index);
                    }
                }
            }
            '\'' | '"' => {
                quote = Some(ch);
                reader.word.get_or_insert(index);
            }
            '$' => {
                if matches!(next, Some(b'\'' | b'"')) {
                    return None;
                }
                let (length, status) = expansion(&command[index..])?;
                reader.step.statuses += usize::from(status);
                skip = index + length;
                reader.word.get_or_insert(index);
            }
            _ => {
                reader.word.get_or_insert(index);
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    reader.end_word(command, command.len())?;
    reader.end_step(None, false)?;
    let reserved = reader
        .steps
        .iter()
        .any(|step| RESERVED_WORDS.contains(&step.words[0]));
    (!reserved).then_some(reader.steps)
}

/// The commands a record shows a command line by: its commands after each
/// leading `cd <directory>` that `&&`, `;`, or a line break joins to a later
/// command, where the step holds no redirection and the directory's text
/// begins with no `-`.
fn after_directory<'s, 'a>(steps: &'s [Step<'a>]) -> &'s [Step<'a>] {
    let mut rest = steps;
    while let [first, later @ ..] = rest {
        let moves = first.words.len() == 2
            && first.words[0] == "cd"
            && !first.redirects
            && !first.words[1]
                .trim_start_matches(['\'', '"', '\\'])
                .starts_with('-')
            && matches!(first.join, Some(Join::And | Join::Then))
            && !later.is_empty();
        if !moves {
            break;
        }
        rest = later;
    }
    rest
}

/// Whether `word` is a `NAME=value` assignment that holds no `<` and no
/// `>`. A word with one of them can carry a redirection, or a value the
/// redactor rewrote, and stays a word of the command.
fn assigns(word: &str) -> bool {
    let name = word
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        .count();
    name > 0
        && !word.as_bytes()[0].is_ascii_digit()
        && word[name..].starts_with('=')
        && !word.contains(['<', '>'])
}

/// The words of a command after its leading `NAME=value` assignments, and
/// all of its words when it holds assignments alone. The first is the
/// program the command runs.
fn program_words<'s, 'a>(step: &'s Step<'a>) -> &'s [&'a str] {
    let lead = step.words.iter().take_while(|word| assigns(word)).count();
    match lead < step.words.len() {
        true => &step.words[lead..],
        false => &step.words,
    }
}

/// `text` with each whitespace run as one space, and without the
/// whitespace at its ends.
fn collapsed(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
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

/// The redactor's text of `text`, or `None` when its scan does not finish.
fn redacted(text: &str) -> Option<String> {
    let scanned = context_core::redaction::redact_windowed_durable_text(text, usize::MAX);
    Some(scanned.ok()?.text)
}

/// What a record shows of a command whose words are `words`: the first
/// when it is a plain word, the second when it is a plain word that begins
/// with no `-`, and [`ARGUMENTS_NOTE`] when the command holds more than
/// the words shown, which `more` states for what follows `words`;
/// [`LEFT_OUT`] when the first is no plain word.
fn shown_of(words: &[&str], more: bool) -> String {
    let Some(program) = words.first().filter(|word| plain_word(word)) else {
        return LEFT_OUT.to_owned();
    };
    let mut shown = (*program).to_owned();
    let operand = words
        .get(1)
        .filter(|word| plain_word(word) && !word.starts_with('-'));
    if let Some(operand) = operand {
        shown.push(' ');
        shown.push_str(operand);
    }
    if more || words.len() > 1 + usize::from(operand.is_some()) {
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
/// project and the whole redacted line, each behind its length. Two
/// spellings of one operation, such as a run with and without a leading
/// `cd` or with other spacing, are two commands with a record each, so a
/// run of one spelling changes the record of that spelling alone.
///
/// The record shows the program and its first plain operand. For a command
/// line the reader follows (see [`commands_of`]) they are the first two
/// words of its first command after the leading `cd` steps (see
/// [`after_directory`]) and the leading assignments (see
/// [`program_words`]). For any other command line the record shows the
/// word it begins with, up to the first whitespace, when that is a plain
/// word and none of [`RESERVED_WORDS`]. Every other argument stays out of
/// the record. The words shown are a reading aid: the digest alone names
/// the command.
fn command_named(scope: &str, command: &str) -> Option<Command> {
    let line = redacted(command)?;
    let mut hash = Sha256::new();
    for part in [scope, line.as_str()] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    let digest = format!("{:x}", hash.finalize())[..32].to_owned();
    let shown = match commands_of(&line) {
        Some(steps) => {
            let steps = after_directory(&steps);
            shown_of(program_words(&steps[0]), steps.len() > 1)
        }
        None => {
            let written = line.trim();
            let first = written
                .split_whitespace()
                .next()
                .filter(|word| !RESERVED_WORDS.contains(word));
            let more = first.is_some_and(|word| word.len() < written.len());
            shown_of(first.as_slice(), more)
        }
    };
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

/// The placeholder of the status parameter in an echo's template. A command
/// line holds no such character in a word that is read.
const STATUS_PLACE: char = '\u{0}';

/// Appends the text the shell prints for one word of an `echo` to `out`,
/// with [`STATUS_PLACE`] for a `$?` or `${?}` inside double quotes, where
/// the shell prints the status whole: the word without its quote characters
/// and without the backslashes that escape a character. Inside double
/// quotes a backslash escapes `$`, `` ` ``, `"`, and itself, and stands as
/// written before any other character. `None` for a word whose printed text
/// depends on more than the exit status: a status parameter outside quotes,
/// which the shell splits at the characters of `IFS`, any other `$` outside
/// single quotes and behind no backslash, and a `*`, `?`, `[`, or `~`
/// outside quotes and behind no backslash, which the shell matches against
/// file names.
fn printed_word(word: &str, out: &mut String) -> Option<()> {
    let mut quote: Option<char> = None;
    let mut skip = 0;
    for (index, ch) in word.char_indices() {
        if index < skip {
            continue;
        }
        let after = index + ch.len_utf8();
        match (quote, ch) {
            (Some(open), _) if ch == open => quote = None,
            (Some('\''), _) => out.push(ch),
            (None, '\'' | '"') => quote = Some(ch),
            (Some(_), '$') => {
                let rest = &word[index..];
                let length = ["$?", "${?}"]
                    .into_iter()
                    .find(|form| rest.starts_with(form))?;
                skip = index + length.len();
                out.push(STATUS_PLACE);
            }
            (None, '$') => return None,
            (_, '\\') => {
                let escaped = word[after..].chars().next()?;
                skip = after + escaped.len_utf8();
                if quote.is_some() && !"$`\"\\".contains(escaped) {
                    out.push('\\');
                }
                out.push(escaped);
            }
            (None, '*' | '?' | '[' | '~') => return None,
            _ if ch == STATUS_PLACE => return None,
            _ => out.push(ch),
        }
    }
    Some(())
}

/// The first words of the commands that end or replace the shell, run code
/// the reader does not see, set code to run at the shell's exit, or run
/// another command by its name. After one of them the last line of the
/// output is no sure line of a later `echo`.
const SHELL_CONTROL: [&str; 10] = [
    "exec", "exit", "return", "logout", "trap", "source", ".", "eval", "command", "builtin",
];

/// Whether a command before the final echo keeps the shell running as far
/// as its text shows: its first word after the words that open with a
/// `NAME=` is a plain word and none of [`SHELL_CONTROL`], or it holds such
/// assignments alone. A first word that is quoted, escaped, expanded, or a
/// redirection names no program the reader can tell.
fn keeps_shell(step: &Step<'_>) -> bool {
    let opens_assignment = |word: &str| {
        let name = word
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            .count();
        name > 0 && !word.as_bytes()[0].is_ascii_digit() && word[name..].starts_with('=')
    };
    step.words
        .iter()
        .find(|word| !opens_assignment(word))
        .is_none_or(|word| plain_word(word) && !SHELL_CONTROL.contains(word))
}

/// The text a command line's final `echo` prints before and after the exit
/// status, when the command line reports its own exit status in the one
/// form the reader follows:
///
/// - its last command is an `echo`, and `;` or a line break joins it to the
///   command before it;
/// - the echo's words hold the command line's only status parameter, which
///   is `$?` or `${?}` inside double quotes;
/// - the echo holds no redirection, the text of its first word after `echo`
///   begins with no `-`, and each of its words prints a fixed text (see
///   [`printed_word`]);
/// - the text before the parameter holds a character other than a digit
///   and a space, so digits that an earlier command left on the line are no
///   part of the number read;
/// - every command before the echo keeps the shell running as far as its
///   text shows (see [`keeps_shell`]).
///
/// The texts are returned with whitespace runs as single spaces. `None`
/// for every other command line, whose exit status the tool's status alone
/// reports. The form is read from the text of the call's command line:
/// code the command line runs from another file, and code a harness runs
/// in the same shell before the command line, can still end the shell or
/// print after the echo.
fn status_echo(steps: &[Step<'_>]) -> Option<(String, String)> {
    let read: usize = steps.iter().map(|step| step.statuses).sum();
    let [earlier @ .., before, last] = steps else {
        return None;
    };
    let follows =
        read == 1 && last.statuses == 1 && !last.redirects && before.join == Some(Join::Then);
    let ["echo", words @ ..] = last.words.as_slice() else {
        return None;
    };
    if !(follows && earlier.iter().chain([before]).all(keeps_shell)) {
        return None;
    }
    let mut template = String::new();
    for (index, word) in words.iter().enumerate() {
        let start = template.len();
        printed_word(word, &mut template)?;
        // An option changes what the echo prints.
        if index == 0 && template[start..].starts_with('-') {
            return None;
        }
        template.push(' ');
    }
    let template = collapsed(&template);
    let (before, after) = template.split_once(STATUS_PLACE)?;
    let labeled = before.chars().any(|ch| !(ch.is_ascii_digit() || ch == ' '));
    labeled.then(|| (before.to_owned(), after.to_owned()))
}

/// The exit status an echo printed in `line`: the line, with whitespace
/// runs as single spaces, is `before`, a number from 0 to 255, and `after`.
fn echoed_status(before: &str, after: &str, line: &str) -> Option<i64> {
    let line = collapsed(line);
    let number = line.strip_prefix(before)?.strip_suffix(after)?;
    let digits =
        !number.is_empty() && number.len() <= 3 && number.bytes().all(|byte| byte.is_ascii_digit());
    let status = digits.then(|| number.parse::<i64>().ok()).flatten()?;
    (status <= 255).then_some(status)
}

/// One number of Pi's truncation note: ASCII digits.
fn count(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// A size as Pi's truncation note prints one: digits and `B`, or digits,
/// `.`, one digit, and `KB` or `MB`.
fn size(text: &str) -> bool {
    match text.strip_suffix("KB").or_else(|| text.strip_suffix("MB")) {
        Some(number) => number
            .split_once('.')
            .is_some_and(|(whole, tenth)| count(whole) && tenth.len() == 1 && count(tenth)),
        None => text.strip_suffix('B').is_some_and(count),
    }
}

/// Whether `path` names a log of Pi's shell tool: a file `pi-bash-`, 16
/// lowercase hexadecimal digits, and `.log`, in any directory.
fn output_log(path: &str) -> bool {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    file.strip_prefix("pi-bash-")
        .and_then(|rest| rest.strip_suffix(".log"))
        .is_some_and(|digits| {
            digits.len() == 16
                && digits
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// What Pi's shell tool cut from the output a note stands after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cut {
    /// Whole lines ahead of the lines shown.
    Lines,
    /// The start of the one line shown, which is a fragment of its line.
    LineStart,
}

/// What the line states when it is the note Pi's shell tool writes after
/// an output it cut, in one of its three forms:
/// `[Showing lines <first>-<last> of <last>. Full output: <path>]`, the
/// same with ` (<size> limit)` before the `.`, and
/// `[Showing last <size> of line <number> (line is <size>). Full output:
/// <path>]`, where the path names a log of the tool (see [`output_log`]).
/// The reading is by the line's text: a command that prints such a line
/// itself is read the same way.
fn truncation_note(line: &str) -> Option<Cut> {
    let (shown, path) = line
        .strip_prefix("[Showing ")?
        .strip_suffix(']')?
        .split_once(". Full output: ")?;
    let lines = || {
        let (range, total) = shown.strip_prefix("lines ")?.split_once(" of ")?;
        let (first, last) = range.split_once('-')?;
        let (total, limit) = match total.split_once(" (") {
            Some((total, limit)) => (total, Some(limit.strip_suffix(" limit)")?)),
            None => (total, None),
        };
        (count(first) && count(last) && last == total && limit.is_none_or(size))
            .then_some(Cut::Lines)
    };
    let line_start = || {
        let (cut, line) = shown.strip_prefix("last ")?.split_once(" of line ")?;
        let (number, whole) = line.split_once(" (line is ")?;
        (size(cut) && count(number) && size(whole.strip_suffix(')')?)).then_some(Cut::LineStart)
    };
    output_log(path)
        .then(|| lines().or_else(line_start))
        .flatten()
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

/// The closing line Pi writes when it ended the command itself: on an
/// abort, on its timeout, and for a shell that ended without an exit code.
/// The command's own outcome stays unknown, so such a result is skipped.
const HARNESS_ENDED: [&str; 4] = [
    "Command aborted",
    "Operation aborted",
    "Command timed out after ",
    "Command terminated without an exit code",
];

/// The whole text of the result Pi writes for a tool call that an extension
/// blocked with Pi's default reason.
const HARNESS_BLOCKED: &str = "Tool execution was blocked";
/// The first line of the result Pi writes for a tool call whose arguments
/// its schema refuses, up to the tool's name.
const HARNESS_INVALID: &str = "Validation failed for tool \"";
/// How the result Pi writes for a tool call it left unrun begins, up to
/// the tool's name, and how it goes on after the name: the model's reply
/// ended at its output limit, so the call's arguments may be cut.
const HARNESS_UNRUN: (&str, &str) = (
    "Tool call \"",
    " was not executed: the response hit the output token limit",
);
/// How the result Pi writes begins when its shell tool refuses the call
/// before it runs the command (a working directory or a configured shell
/// that is missing, a timeout argument it does not take), and when an
/// extension's hook threw a value that is no error object before the tool
/// ran. A hook that throws an error object gives the result that error's
/// own message, which reads as the output of a failed command.
const HARNESS_UNSTARTED: [&str; 4] = [
    "Working directory does not exist: ",
    "Custom shell path not found: ",
    "Invalid timeout: ",
    "Extension failed, blocking execution: ",
];

/// Whether the harness ended or refused the command itself, which leaves
/// the command's own outcome unknown: the result carries the harness's
/// error mark and its last non-empty line begins with one of
/// [`HARNESS_ENDED`], its whole text is [`HARNESS_BLOCKED`],
/// `Tool <name> not found`, or `spawn <shell> <ERRNO>`, or it begins with
/// [`HARNESS_INVALID`], with one of [`HARNESS_UNSTARTED`], or with
/// [`HARNESS_UNRUN`] around a tool's name. The forms are the texts Pi writes, read as written: the reading
/// is by text, so a marked result of a command that prints one of them in
/// that place is skipped too.
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
    let unrun = text
        .strip_prefix(HARNESS_UNRUN.0)
        .and_then(|rest| rest.split_once('"'))
        .is_some_and(|(_, rest)| rest.starts_with(HARNESS_UNRUN.1));
    let unstarted = HARNESS_UNSTARTED.iter().any(|mark| text.starts_with(mark));
    // The whole text Pi's shell tool writes when the shell did not start.
    let unspawned = text
        .strip_prefix("spawn ")
        .filter(|rest| !rest.contains('\n'))
        .and_then(|rest| rest.rsplit_once(' '))
        .is_some_and(|(shell, errno)| {
            !shell.trim().is_empty()
                && errno.len() > 1
                && errno.starts_with('E')
                && errno
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        });
    ended
        || unrun
        || unstarted
        || unspawned
        || text.starts_with(HARNESS_INVALID)
        || text == HARNESS_BLOCKED
        || missing
}

/// What a command's result states of its outcome, before the redactor has
/// judged the output.
///
/// A record states the exit status of its command as the tool reports it.
/// The harness's error mark is a failure, and its code is read from the
/// last non-empty line alone, which is where the harness states it (see
/// [`stated_exit_code`]).
///
/// A result without the mark reports no failure, with two exceptions, each
/// a statement of an exit status in the output. A command line whose final
/// `echo` prints its exit status (see [`status_echo`]) reports by that
/// number alone, which the shell printed for the commands before the echo.
/// The number is read from the output's last non-empty line: a nonzero one
/// is a failure, a zero is a pass, and an output whose last line is no line
/// of the echo states no outcome. A command line whose last command is an
/// `echo` of a status parameter in any other form reports by the tool's
/// status alone, since the line that echo prints carries digits the shell
/// may have changed. For any other command line a status line decides, as
/// a tool or a runner states its status without the harness's mark: the
/// lines read are the first non-empty line, when it ends within the
/// output's first [`MAX_FIRST_LINE_BYTES`], and the last
/// [`EXIT_CODE_CLOSING_LINES`] non-empty lines; the last of the closing
/// lines that states a code decides, and the first line when none of them
/// states one. A nonzero code is a failure and a zero is a pass. Both
/// readings are by the line's text.
///
/// The shell reports a pipeline or a list by the status it returns for the
/// whole, which by default is the last command's. A failure of an earlier
/// command that the returned status leaves out is outside what a record
/// states, whatever else the output holds.
///
/// The chosen lines are the first lines that state an error, each failure
/// line with the line that completes it, and then the closing lines of the
/// output. A line with a control character or a Unicode line separator is
/// left out, a text that several lines share is chosen once, and Pi's note
/// after an output it cut (see [`truncation_note`]) is read as no line.
/// The line before a note of a line's cut start is a fragment of its line:
/// it states no exit code and is no line of an echo.
fn outcome_of(command: &str, text: &str, flagged: bool) -> Reported {
    let mut start = 0;
    // Each line, trimmed, with the bytes it stands at.
    let mut lines: Vec<(&str, Range<usize>)> = Vec::with_capacity(text.split('\n').count());
    for line in text.split('\n') {
        let span = start..start + line.len();
        start = span.end + 1;
        lines.push((line.trim(), span));
    }
    // The harness writes its note last, or before the status line it adds
    // to a failed result. The line before a note of a line's cut start is a
    // fragment, which states no status.
    let mut fragment = None;
    let closing: Vec<usize> = (0..lines.len())
        .rev()
        .filter(|index| !lines[*index].0.is_empty())
        .take(3)
        .collect();
    for (place, index) in closing.iter().enumerate().take(2) {
        if let Some(cut) = truncation_note(lines[*index].0) {
            lines[*index].0 = "";
            if cut == Cut::LineStart {
                fragment = closing.get(place + 1).copied();
            }
        }
    }
    let shown = |line: &str| {
        !line.is_empty() && printable(line) && !line.contains(['\u{2028}', '\u{2029}'])
    };
    // The lines that can state a status, each with its place.
    let whole = || {
        lines
            .iter()
            .enumerate()
            .filter(|(index, (line, _))| !line.is_empty() && Some(*index) != fragment)
            .map(|(_, line)| line)
    };
    let states =
        |(line, span): &(&str, Range<usize>)| Some((stated_exit_code(line)?, span.clone()));
    let steps = commands_of(command);
    let echo = steps.as_deref().and_then(status_echo);
    // A final echo of a status parameter is the command line's own report
    // of its status, also in a form the reader does not follow.
    let reports = steps
        .as_deref()
        .and_then(<[Step<'_>]>::last)
        .is_some_and(|last| last.words[0] == "echo" && last.statuses > 0);
    let stated = {
        let mut closing = lines
            .iter()
            .enumerate()
            .rev()
            .filter(|(index, (line, _))| !line.is_empty() && Some(*index) != fragment)
            .map(|(_, line)| line);
        match (flagged, reports) {
            (true, _) => closing.next().and_then(states),
            (false, true) => None,
            (false, false) => {
                let first = whole()
                    .next()
                    .filter(|(_, span)| span.end <= MAX_FIRST_LINE_BYTES);
                closing
                    .take(EXIT_CODE_CLOSING_LINES)
                    .find_map(states)
                    .or_else(|| first.and_then(states))
            }
        }
    };
    let printed = echo.as_ref().and_then(|(before, after)| {
        let (index, (line, span)) = lines
            .iter()
            .enumerate()
            .rev()
            .find(|(_, (line, _))| !line.is_empty())?;
        (Some(index) != fragment)
            .then(|| Some((echoed_status(before, after, line)?, span.clone())))
            .flatten()
    });
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
        flagged,
        stated,
        echoes: echo.is_some(),
        printed,
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

/// What a result says of its command once the redactor has judged the
/// output `text` that `reported` was read from: a failure, a pass, or
/// `None` for a result that changes no record.
///
/// A result the harness marks is a failure, with the code its last line
/// states when the redactor finds nothing in that line. A command line that
/// reports by its final `echo` is judged by the number that echo printed,
/// when the redactor finds nothing in its line: a nonzero one is a failure
/// and a zero is a pass. Any other result is judged by its deciding status
/// line, when the redactor finds nothing in that line: a nonzero code is a
/// failure and a zero is a pass, and a result that states no code is a
/// pass. A result whose deciding line the redactor refuses, or whose
/// output it left unscanned, changes no record: it is no failure and no
/// pass. So does a command line whose echo printed no last line.
///
/// The text kept follows source capture's rule for a tool span: text in
/// which the redactor finds nothing is kept as it is, and text with a
/// finding is withheld whole. With no finding the chosen lines stand in
/// the record. With a finding the record holds no line, and the path and
/// the error name come from lines that hold no finding; when the scan does
/// not finish, no text of the output reaches the record.
fn screened(reported: Reported, text: &str) -> Option<Outcome> {
    let Reported {
        flagged,
        stated,
        echoes,
        printed,
        lines,
    } = reported;
    if !(flagged || stated.is_some() || echoes) {
        return Some(Outcome::Passed);
    }
    let findings = findings_in(text);
    let clear = |span: &Range<usize>| {
        findings.as_ref().is_some_and(|findings| {
            findings
                .iter()
                .all(|finding| finding.end <= span.start || span.end <= finding.start)
        })
    };
    let clean = |code: &Option<(i64, Range<usize>)>| {
        code.as_ref()
            .filter(|(_, span)| clear(span))
            .map(|(code, _)| *code)
    };
    let (evidence, exit_code) = match (flagged, echoes) {
        (true, _) => (Evidence::Marked, clean(&stated).filter(|code| *code != 0)),
        (false, true) => match clean(&printed)? {
            0 => return Some(Outcome::Passed),
            code => (Evidence::Printed, Some(code)),
        },
        (false, false) => match clean(&stated)? {
            0 => return Some(Outcome::Passed),
            code => (Evidence::Stated, Some(code)),
        },
    };
    let (path, error) = named_in(&lines, text, clear);
    let excerpt = match findings.as_deref() {
        Some([]) => Excerpt::Kept(lines.into_iter().map(|(line, _)| line).collect()),
        Some(_) => Excerpt::Finding,
        None => Excerpt::Unscanned,
    };
    Some(Outcome::Failed(Failure {
        evidence,
        exit_code,
        path,
        error,
        excerpt,
    }))
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
        (Evidence::Marked, Some(code)) => format!("the command failed with exit code {code}"),
        (Evidence::Marked, None) => "the command failed and the tool reported an error".to_owned(),
        (Evidence::Printed, code) => format!(
            "the command failed: its command line printed exit status {}",
            code.unwrap_or_default()
        ),
        (Evidence::Stated, code) => format!(
            "the command failed: a status line of its output states exit code {}",
            code.unwrap_or_default()
        ),
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
            text,
            flagged,
            at_ms,
        })
    }

    /// The bytes this copy holds.
    fn bytes(&self) -> usize {
        self.call.len() + self.tool.len() + self.command.len() + self.text.len()
    }

    /// Reads the command and the result's outcome. `Err` for a result that
    /// changes no record, with the reason when an operator can act on it:
    /// a tool name or a call id that the redactor would change or that
    /// holds a control character or a Unicode line separator, and a command
    /// line whose redactor scan does not finish. A result the harness
    /// refused or ended itself, and one whose outcome stays unstated (see
    /// [`screened`]), change no record for no such reason. The tool's name
    /// and the call's id are judged whole, before they are cut for display.
    fn observe(&self, scope: &str) -> Result<Observed, Option<&'static str>> {
        if harness_decided(&self.text, self.flagged) {
            return Err(None);
        }
        if !(names_whole(&self.tool) && names_whole(&self.call)) {
            return Err(Some(
                "the redactor changes the tool's name or the call's id, or one of them holds a control character",
            ));
        }
        let command = command_named(scope, &self.command).ok_or(Some(
            "the redactor's scan of the command line did not finish",
        ))?;
        let reported = outcome_of(&self.command, &self.text, self.flagged);
        let outcome = screened(reported, &self.text).ok_or(None)?;
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
                Some(input) if session_whole => input.observe(writer.scope),
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

    /// The ground, the exit code, and the kept lines of a failed text
    /// result of `command`: `None` for a pass and for a result that states
    /// no outcome.
    fn failed(
        command: &str,
        text: &str,
        flagged: bool,
    ) -> Option<(Evidence, Option<i64>, Vec<String>)> {
        match screened(outcome_of(command, text, flagged), text)? {
            Outcome::Passed => None,
            Outcome::Failed(failure) => {
                let lines = match failure.excerpt {
                    Excerpt::Kept(lines) => lines.into_iter().map(|line| line.text).collect(),
                    Excerpt::Finding | Excerpt::Unscanned => Vec::new(),
                };
                Some((failure.evidence, failure.exit_code, lines))
            }
        }
    }

    /// What an unmarked text result says of `command`.
    #[derive(Debug, PartialEq, Eq)]
    enum Said {
        Pass,
        Fail(Evidence, Option<i64>),
        /// The result states no outcome and changes no record.
        Nothing,
    }

    fn said(command: &str, text: &str) -> Said {
        match screened(outcome_of(command, text, false), text) {
            None => Said::Nothing,
            Some(Outcome::Passed) => Said::Pass,
            Some(Outcome::Failed(failure)) => Said::Fail(failure.evidence, failure.exit_code),
        }
    }

    fn named(command: &str) -> Command {
        command_named(SCOPE, command).unwrap()
    }

    /// The texts around the status of the command line's final echo, when
    /// the reader follows it.
    fn status_echo_of(command: &str) -> Option<(String, String)> {
        status_echo(&commands_of(command)?)
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
        match input(command, text, flagged).observe(SCOPE).ok()?.outcome {
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
            Some(("one\ntwo".to_owned(), true))
        );
        assert_eq!(
            text_of(&OutputKind::Content {
                blocks: vec![words("(no output)")],
            }),
            Some(("(no output)".to_owned(), false))
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
            Some(("\n".to_owned(), false))
        );
        assert_eq!(
            text_of(&OutputKind::Content {
                blocks: vec![words(" \n")]
            }),
            Some((" \n".to_owned(), false))
        );
        assert_eq!(said("make check", "\n"), Said::Pass);
        assert_eq!(
            said("make check; echo \"rc=$?\"", "\n"),
            Said::Nothing,
            "the echo's line is absent"
        );
        // A text that is one JSON object is text: its exit field decides
        // nothing, since a passing command can print one.
        let object = json!({ "exitCode": 137, "reason": "OOMKilled" }).to_string();
        assert_eq!(said("kubectl get pod api -o json", &object), Said::Pass);
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
            // A command line outside the reader's grammar shows the plain
            // word it begins with, and no word from inside a group.
            ("FILES=(a.txt b.txt c.txt); wc -l a.txt", LEFT_OUT),
            ("NAME=${NAME:-John Q Public} ./greet", LEFT_OUT),
            ("diff <(sort a) <(sort b)", "diff [arguments left out]"),
            (
                "python3 - <<'PY'\nprint(1)\nPY",
                "python3 [arguments left out]",
            ),
            ("if make; then ls; fi", LEFT_OUT),
            // A line break parts two commands.
            ("echo\nfalse", "echo [arguments left out]"),
            // The words shown leave out a leading directory step and the
            // leading assignments.
            (
                "cd services && cargo test -p api",
                "cargo test [arguments left out]",
            ),
            (
                "cd 'my services'; RUST_LOG=debug cargo  test -p api",
                "cargo test [arguments left out]",
            ),
            (
                "RUST_LOG=debug TZ=UTC cargo test -p api",
                "cargo test [arguments left out]",
            ),
            ("cd a && \\\n  cargo test", "cargo test"),
            ("cd services", "cd services"),
            // A directory step with an option, a redirection, or a word
            // that is no plain word, and an assignment that carries a
            // redirection, stay words of the command.
            ("cd -P && make check", "cd [arguments left out]"),
            ("cd '-Z' && true", "cd [arguments left out]"),
            ("cd \\-Z && true", "cd [arguments left out]"),
            ("cd >missing/log && make check", "cd [arguments left out]"),
            ("cd 2>/dev/null && make check", "cd [arguments left out]"),
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
        use Evidence::Marked;
        let lines = |lines: &[&str]| lines.iter().map(|line| (*line).to_owned()).collect();
        // The harness's mark decides, also against a stated code of zero,
        // and its code is read from the last non-empty line alone.
        assert_eq!(
            failed("make", "ok\n\nCommand exited with code 2\n", true),
            Some((
                Marked,
                Some(2),
                lines(&["ok", "Command exited with code 2"])
            ))
        );
        assert_eq!(
            failed("make", "exit code 3\nall done", true),
            Some((Marked, None, lines(&["exit code 3", "all done"])))
        );
        assert_eq!(
            failed("make", "Exit code: 0", true),
            Some((Marked, None, lines(&["Exit code: 0"])))
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
    fn a_command_line_is_read_as_commands_or_left_unread() {
        let joins = |command: &str| {
            commands_of(command).map(|steps| {
                steps
                    .iter()
                    .map(|step| (step.words.join(" "), step.join))
                    .collect::<Vec<_>>()
            })
        };
        let step = |words: &str, join| (words.to_owned(), join);
        assert_eq!(
            joins("cd api && make check 2>&1 | tail -n 5; ls 'a b' || true\n"),
            Some(vec![
                step("cd api", Some(Join::And)),
                step("make check 2>&1", Some(Join::Pipe)),
                step("tail -n 5", Some(Join::Then)),
                step("ls 'a b'", Some(Join::Or)),
                step("true", None),
            ])
        );
        // An operator in quotes or behind a backslash is a word's text, a
        // line break after an operator joins nothing, and the `&` of a
        // redirection is part of its word.
        for (command, words) in [
            ("grep -E 'error|warning' \"a; b\" c\\;d", 5),
            ("make check &&\n  ls", 3),
            ("make check > out.log 2>&1", 5),
            ("make check &> out.log", 4),
            ("make check >&2", 3),
            ("cat <&3", 2),
            ("echo ${HOME}/bin $USER \"${PWD}\" $# $1 $", 7),
            // A comment runs from a `#` that begins a word to the end of
            // its line, quotes and operators in it included.
            ("make check # the default target; don't | stop", 2),
            ("# build\nmake check\n  # then list\nls", 3),
            ("echo a#b \\# '#' c", 5),
            // A backslash and line break between two words joins the
            // lines, also before a comment.
            ("make \\\n  check", 2),
            ("cd api && \\\nmake check", 4),
            ("grep x build.log \\\n# note; true", 3),
            // An escaped `>` is a word's text, so a pipe after it is a
            // pipe.
            ("echo marker\\>| cat", 3),
        ] {
            let steps = commands_of(command).unwrap();
            let read: usize = steps.iter().map(|step| step.words.len()).sum();
            assert_eq!(read, words, "{command}");
        }
        assert_eq!(
            commands_of("make >&2; ls").map(|steps| steps[0].redirects),
            Some(true)
        );
        assert_eq!(
            joins("echo marker\\>| cat").map(|steps| steps.len()),
            Some(2)
        );
        for outside in [
            "python3 - <<'PY'\nprint('a | b')\nPY",
            "cat <<EOF\nbody\nEOF",
            "cat <<< text",
            "make check | tee >(grep error)",
            "diff <(sort a) <(sort b)",
            "(make check; true)",
            "{ make check; true; }",
            "make check & wait",
            "make check &",
            "make check \\>& true",
            "grep x \\<& true",
            "make check $(cat flags)",
            "make \"$(cat flags)\"",
            "make check `cat flags`",
            "make \"`cat flags`\"",
            "MODE=$[1 + 2] make check",
            "echo \"$[1+2]\"",
            "if make check; then ls; fi",
            "for f in a b; do make $f; done",
            "! make check",
            "true && \\\n! make check",
            "[[ -f Makefile ]] && make",
            "make check |& tail",
            "make check >| out.log",
            "echo ${OUT:-a | b}",
            "echo \"${OUT:-a}\"",
            "echo $'it\\'s' ; make check",
            "echo $\"text\"",
            "make check ||",
            "make check | | tail",
            "; make check",
            "make check ;; ls",
            "make 'check",
            "make \"check",
            "make check \\",
            "make che\\\nck",
            "echo \"a \\\n b\"",
            "test -f\r build.stamp",
            "find . -name x -exec cat {} \\;",
            "FILES=(a.txt b.txt); wc -l a.txt",
        ] {
            assert_eq!(commands_of(outside), None, "{outside:?}");
        }
        // The reader's tables hold at most 64 commands and 1,024 words.
        let many = |count: usize, part: &str| format!("{}make", part.repeat(count));
        assert_eq!(
            commands_of(&many(63, "ls; ")).map(|steps| steps.len()),
            Some(64)
        );
        assert_eq!(commands_of(&many(64, "ls; ")), None);
        assert!(commands_of(&many(1_023, "x ")).is_some());
        assert_eq!(commands_of(&many(1_024, "x ")), None);
        // A status parameter is counted where the shell expands it.
        let statuses = |command: &str| {
            commands_of(command)
                .unwrap()
                .iter()
                .map(|step| step.statuses)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            statuses("make; echo \"$? ${?}\" '$?' \\$? ${PIPESTATUS[0]} ${PIPESTATUS[@]}"),
            [0, 4]
        );
        assert_eq!(
            statuses("echo ${PIPESTATUS} $PIPESTATUS \"$PIPESTATUS\""),
            [3]
        );
        assert_eq!(statuses("echo $PIPESTATUSES ${PIPESTATUS_2}"), [0]);
        // `$$` is the shell's process id, and a `?` after it is text.
        assert_eq!(statuses("echo \"$$?\" $$? $$PIPESTATUS $$$? \\$$?"), [2]);
        assert_eq!(statuses("make # echo $?\nls"), [0, 0]);
        assert_eq!(commands_of("echo ${PIPESTATUS[x]}"), None);
    }

    #[test]
    fn a_final_labeled_echo_of_the_exit_status_reports_for_its_command_line() {
        use Said::{Fail, Nothing, Pass};
        const ERROR: &str = "error: could not compile `api`";
        let printed = |code| Fail(Evidence::Printed, Some(code));
        for (command, text, code) in [
            ("make check; echo \"exit=$?\"", "exit=2", 2),
            ("make check; echo \"exit=$?\"", "work\nexit=2\n", 2),
            ("make check; echo \"EXIT CODE: ${?}\"", "EXIT CODE: 3", 3),
            (
                "make check; echo \"status $? reported\"",
                "bye\nstatus 4 reported",
                4,
            ),
            ("cd api && make check; echo \"rc=$?\"", "rc=2", 2),
            ("make check\necho \"rc=$?\"\n", "rc=255\n\n", 255),
            ("make check | tail -n 3; echo \"exit=$?\"", "exit=9", 9),
            // A parameter in single quotes or behind a backslash is text
            // the echo prints as written, beside the one that expands.
            ("false; echo '$?' \"$?\"", "$? 1", 1),
            ("false; echo \"\\$? is $?\"", "$? is 7", 7),
            ("false; echo \\* \"rc=$?\"", "* rc=3", 3),
            // A first word that is empty once its quotes are gone is no
            // option.
            ("make check; echo \"\" -n \"rc=$?\"", " -n rc=1", 1),
            // The echo's label is the command line's own text, also where
            // it reads as a status line: the number is the parameter's.
            ("false; echo \"exit code 1$?\"", "exit code 11", 1),
            ("make check; echo \"exit code: $?\"", "exit code: 2", 2),
            // Pi's note after an output it cut is no line of the command.
            (
                "make check; echo \"rc=$?\"",
                "rc=2\n\n[Showing lines 9-40 of 40. Full output: /tmp/pi-bash-0123456789abcdef.log]",
                2,
            ),
        ] {
            assert_eq!(said(command, text), printed(code), "{command}");
        }
        // A printed zero is a pass, whatever else the output holds, a
        // status line above it included.
        for command in [
            "make check; echo \"exit=$?\"",
            "make check | tail -n 40; echo \"exit=$?\"",
        ] {
            assert_eq!(said(command, "exit=0"), Pass, "{command}");
            for above in [ERROR, "Process exited with code 1"] {
                assert_eq!(
                    said(command, &format!("{above}\nexit=0")),
                    Pass,
                    "{command}"
                );
            }
        }
        assert_eq!(said("true; echo \"exit code 1$?\"", "exit code 10"), Pass);
        assert_eq!(
            said("make check; echo \"exit code: $?\"", "exit code: 0"),
            Pass
        );
        // The output's last line is no line of the echo: the result states
        // no outcome, so it records no failure and resolves none.
        for text in [
            "exit=two",
            "the exit=2 line",
            "exit=2\ndone",
            "exit=256",
            "exit=0007",
            "exit=1 2",
            "exit=",
            "exit code 2",
        ] {
            assert_eq!(
                said("make check; echo \"exit=$?\"", text),
                Nothing,
                "{text}"
            );
        }
        // Every other command line reports by the tool's status alone: the
        // echo may not run, may print for another command, prints a status
        // the shell can split, or prints text that digits of an earlier
        // command's output can complete.
        for (command, text) in [
            // No label before the number, or one of digits alone.
            ("make check; echo \"$?\"", "work\n5"),
            ("printf 200; echo \"$?\"", "2000"),
            ("cat .nvmrc; echo \"$? done\"", "203 done"),
            ("cat .nvmrc; echo \"  $?\"", "20"),
            ("printf 1; echo \"1$?\"", "110"),
            // A status parameter outside quotes is split at the characters
            // of `IFS`.
            ("make check; echo rc=$?", "rc=1"),
            ("IFS=1; make check; echo rc: $?", "rc: 0"),
            // A final echo of the status that the reader does not follow
            // is still the command line's own report: its line is read as
            // no status line either.
            ("IFS=1; make check; echo exit_code=$?", "exit_code=2"),
            ("true; echo exit_code=1$?", "exit_code=10"),
            ("make check; echo exit code $?", "exit code 2"),
            ("false; echo \\* rc=$?", "* rc=3"),
            // The echo is joined by an operator that can skip it or feeds
            // it a pipe.
            ("grep -c TODO notes.txt || echo \"rc=$?\"", "rc=3"),
            ("make check && echo \"rc=$?\"", "rc=0"),
            ("make check | echo \"rc=$?\"", "rc=1"),
            ("echo \"rc=$?\"", "rc=1"),
            // An earlier command ends or replaces the shell, runs code the
            // reader does not see, or sets code to run at the shell's exit.
            ("exec tail -n 1 build.log; echo \"rc=$?\"", "rc=1"),
            ("printf 'rc=7\\n'; exit 0; echo \"rc=$?\"", "rc=7"),
            ("FOO=1 exec make; echo \"rc=$?\"", "rc=1"),
            (
                "test -f quiet && trap 'echo rc=0' EXIT; false; echo \"rc=$?\"",
                "rc=1\nrc=0",
            ),
            ("source env.sh; make; echo \"rc=$?\"", "rc=1"),
            (". ./env.sh; make; echo \"rc=$?\"", "rc=1"),
            ("eval make; echo \"rc=$?\"", "rc=1"),
            ("return 3; echo \"rc=$?\"", "rc=1"),
            ("logout; echo \"rc=$?\"", "rc=1"),
            ("command exec make; echo \"rc=$?\"", "rc=7"),
            ("builtin exit 0; echo \"rc=$?\"", "rc=7"),
            // The first word of an earlier command is quoted, escaped,
            // expanded, or a redirection, so it names no program the
            // reader can tell.
            ("\"exit\" 0; echo \"rc=$?\"", "rc=7"),
            ("\\exit 0; echo \"rc=$?\"", "rc=7"),
            ("'exec' tail -n 1 log; echo \"rc=$?\"", "rc=7"),
            ("MODE='a>b' exit 0; echo \"rc=$?\"", "rc=7"),
            ("FOO=1>log exec make; echo \"rc=$?\"", "rc=7"),
            ("2>/dev/null exec tail; echo \"rc=$?\"", "rc=7"),
            ("exec>/dev/null; echo \"rc=$?\"", "rc=7"),
            ("$RUNNER check; echo \"rc=$?\"", "rc=7"),
            // An option changes what the echo prints, in any spelling.
            ("make check; echo -n \"rc=$?\"", "rc=1"),
            ("make check; echo \"-n\" \"rc=$?\"", "rc=1"),
            ("make check; echo '-n' \"rc=$?\"", "rc=1"),
            ("make check; echo \\-n \"rc=$?\"", "rc=1"),
            ("make check; echo \"-\"n \"rc=$?\"", "rc=1"),
            // The command line reads its status in another place too, or
            // in another form.
            ("false; echo \"rc=$?\"; true; echo \"rc=$?\"", "rc=1\nrc=1"),
            ("cd \"$?\"; make check; echo \"rc=$?\"", "rc=1"),
            ("make check; rc=$?; echo \"rc=$rc\"", "rc=1"),
            ("make check; echo \"rc=$?\"; ls", "rc=1\nMakefile"),
            ("make check; echo \"$? $?\"", "2 2"),
            ("make check | tail; echo \"rc=${PIPESTATUS[0]}\"", "rc=1"),
            ("make check | tail; echo \"rc=$PIPESTATUS\"", "rc=1"),
            ("make check; printf 'exit=%s' \"$?\"", "exit=2"),
            ("make check; echo \"exit=$?\" >&2", "exit=7"),
            ("make check; echo \"rc=$?\" 0</dev/null", "rc=1"),
            ("make check; echo \"rc=$? in $PWD\"", "rc=1 in /srv"),
            ("make check; echo \"rc=$?\" *", "rc=1 a b"),
            ("make check; echo \"rc=$?\" \"$\"", "rc=1 $"),
            ("(make check; echo \"exit=$?\") | cat", "exit=8"),
            ("make check; echo 'exit=$?'", "exit=2"),
            ("make check; echo \"exit=\\$?\"", "exit=2"),
            ("make check", "exit=2"),
        ] {
            assert_eq!(status_echo_of(command), None, "{command}");
            assert_eq!(said(command, text), Pass, "{command}: {text}");
        }
        // A command of assignments alone, and one under assignments, keep
        // the shell running.
        assert_eq!(
            said("MODE=fast; FOO=1 make check; echo \"rc=$?\"", "rc=2"),
            printed(2)
        );
        // The echo's text and the output's line are compared with their
        // whitespace runs as single spaces.
        assert_eq!(
            said("make check; echo \"exit   code:  $?\"", "exit code:   2"),
            printed(2)
        );
        // The process id's parameter is no status parameter, so the one
        // echo of the status after it is followed.
        assert_eq!(
            said("echo \"pid=$$?\"; false; echo \"rc=$?\"", "pid=41?\nrc=1"),
            printed(1)
        );
        assert_eq!(
            said("echo pid=$$?; false; echo \"rc=$?\"", "pid=41?\nrc=1"),
            printed(1)
        );
    }

    #[test]
    fn an_unmarked_result_fails_by_a_stated_exit_code_and_passes_without_one() {
        use Said::Pass;
        const ERROR: &str = "error: could not compile `api`";
        // A record states the exit status the tool reports for the command
        // line. The shell reports a pipeline or a list by the status it
        // returns for the whole, so a failure line, a runner's summary, or
        // a shell's report in the output of an unmarked result states no
        // failure, for one command and behind a pipe or a list alike.
        for command in [
            "make check",
            "grep -rn error: logs/",
            "cd api && make check",
            "make check 2>&1 | tail -n 40",
            "make check || true",
            "make check; ls target",
            "pnpm typecheck | tee check.log",
            "python3 - <<'PY'\nprint('a | b')\nPY",
            "bash -lc 'make check; make lint'",
            "set -euo pipefail\nnpm ci\nnpm test",
        ] {
            for text in [
                ERROR,
                "bash: line 1: pnpm: command not found",
                "test result: FAILED. 3 passed; 1 failed",
                "Exit code: 0",
                "ok",
            ] {
                assert_eq!(said(command, text), Pass, "{command}: {text}");
            }
            // A status line with a nonzero code is a statement of the
            // command's status by a tool or a runner, with the harness's
            // mark and without it.
            assert_eq!(
                failed(command, "Command exited with code 2", true).map(|failure| failure.1),
                Some(Some(2)),
                "{command}"
            );
            for text in [
                format!("{ERROR}\nexit code 2"),
                "work\n ELIFECYCLE  Command failed with exit code 2.".to_owned(),
            ] {
                assert_eq!(
                    said(command, &text),
                    Said::Fail(Evidence::Stated, Some(2)),
                    "{command}: {text}"
                );
            }
        }
        // The shell returns a pipeline's failure under `pipefail`, and the
        // harness marks it.
        assert_eq!(
            failed(
                "set -o pipefail; make check | tail",
                "Command exited with code 2",
                true
            )
            .map(|failure| failure.1),
            Some(Some(2))
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
            "error: slow\n\nCommand timed out after 120 seconds",
            "partial output\n\nCommand terminated without an exit code",
            "Tool call \"bash\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments.",
            "Working directory does not exist: /srv/gone\nCannot execute bash commands.",
            "Custom shell path not found: /missing/bash",
            "Invalid timeout: must be a finite number of seconds",
            "Invalid timeout: maximum is 2147483 seconds",
            "Extension failed, blocking execution: hook timed out",
            "spawn /bin/bash EACCES",
            "spawn /bin/bash EAGAIN",
            "spawn /opt/dev tools/bash EACCES\n",
        ] {
            assert!(harness_decided(text, true), "{text}");
            assert!(!harness_decided(text, false), "unmarked: {text}");
            assert_eq!(
                input("make", text, true).observe(SCOPE).err(),
                Some(None),
                "{text}"
            );
        }
        for text in [
            "Command aborted\nand then more output",
            "Tool execution was blocked by policy",
            "Tool bash and zsh not found",
            "error: Validation failed for tool \"bash\"",
            "Command exited with code 1",
            "Tool call \"bash\" was executed",
            "Tool call bash was not executed: the response hit the output token limit",
            "error: Working directory does not exist: /srv/gone",
            "make: Invalid timeout: 5x",
            "Error: spawn /bin/bash EACCES\n\nCommand exited with code 2",
            "spawn /bin/bash failed",
            "spawn /bin/bash EACCES again",
            "spawn /bin/bash E",
            "spawn EACCES",
            "spawn worker\nchild ENOENT",
            "Tool execution aborted",
        ] {
            assert!(!harness_decided(text, true), "{text}");
            assert!(input("make", text, true).observe(SCOPE).is_ok(), "{text}");
        }
    }

    #[test]
    fn pi_s_note_after_a_cut_output_is_read_as_no_line() {
        const LOG: &str = "/tmp/pi-bash-0123456789abcdef.log";
        for (note, cut) in [
            (
                format!("[Showing lines 3-9 of 9. Full output: {LOG}]"),
                Cut::Lines,
            ),
            (
                format!("[Showing lines 3-9 of 9 (50.0KB limit). Full output: {LOG}]"),
                Cut::Lines,
            ),
            (
                "[Showing lines 3-9 of 9. Full output: C:\\Temp\\pi-bash-0123456789abcdef.log]"
                    .to_owned(),
                Cut::Lines,
            ),
            (
                format!("[Showing last 50.0KB of line 9 (line is 1.2MB). Full output: {LOG}]"),
                Cut::LineStart,
            ),
            (
                format!("[Showing last 512B of line 1 (line is 900B). Full output: {LOG}]"),
                Cut::LineStart,
            ),
        ] {
            assert_eq!(truncation_note(&note), Some(cut), "{note}");
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
                        ("last line", true),
                        ("Command exited with code 1", true)
                    ]
                    .map(|(text, closing)| Kept {
                        text: text.to_owned(),
                        closing
                    })
                    .to_vec()
                )
            );
            // The note of an unmarked result is its last line. After a
            // note of whole lines the line before it states its code, and
            // after a note of a line's cut start that line is a fragment,
            // which states no status and is no line of an echo.
            let stated = failed("make", &format!("work\n\nexit code 4\n\n{note}"), false)
                .map(|failure| failure.1);
            let echoed = said("make check; echo \"rc=$?\"", &format!("rc=2\n\n{note}"));
            match cut {
                Cut::Lines => {
                    assert_eq!(stated, Some(Some(4)), "{note}");
                    assert_eq!(echoed, Said::Fail(Evidence::Printed, Some(2)), "{note}");
                }
                Cut::LineStart => {
                    assert_eq!(stated, None, "{note}");
                    assert_eq!(echoed, Said::Nothing, "{note}");
                }
            }
        }
        // A marked result keeps the code of the harness's own line after a
        // fragment and its note.
        let cut_line = format!(
            "   exit code 7\n\n[Showing last 50.0KB of line 1 (line is 1.2MB). Full output: {LOG}]\n\nCommand exited with code 2"
        );
        assert_eq!(
            failed("make", &cut_line, true).map(|failure| failure.1),
            Some(Some(2))
        );
        // A line that only looks like a note is a line of the output: after
        // a stated code and two more lines, the code stands outside the
        // last three lines and the result is a pass.
        for line in [
            "[Showing results]".to_owned(),
            format!("Showing lines 3-9 of 9. Full output: {LOG}"),
            "[Showing lines 3-9 of 9]".to_owned(),
            format!("[Showing lines of code. Full output: {LOG}]"),
            format!("[Showing lines 3-9 of many. Full output: {LOG}]"),
            format!("[Showing lines 3-8 of 9. Full output: {LOG}]"),
            format!("[Showing lines 3-9 of 9 (big limit). Full output: {LOG}]"),
            "[Showing lines 3-9 of 9. Full output: ]".to_owned(),
            "[Showing lines 3-9 of 9. Full output: /tmp/out.log]".to_owned(),
            "[Showing lines 3-9 of 9. Full output: /tmp/pi-bash-0123456789ABCDEF.log]".to_owned(),
            "[Showing lines 3-9 of 9. Full output: /tmp/pi-bash-0123.log]".to_owned(),
            format!("[Showing last 50KB of line 9 (line is 1.2MB). Full output: {LOG}]"),
            format!("[Showing last ERROR of line 9 (line is 1.2MB). Full output: {LOG}]"),
        ] {
            assert_eq!(truncation_note(&line), None, "{line}");
            let after_code = format!("start\nexit code 7\none\ntwo\n{line}");
            assert_eq!(failed("make", &after_code, false), None, "{line}");
        }
        // The note is read as one among the last two lines alone: third
        // from the last it is a line of the output, so a stated code above
        // it stands outside the last three lines.
        let note = format!("[Showing lines 1-9 of 9. Full output: {LOG}]");
        let third = format!("work\nexit code 7\n{note}\none\ntwo");
        assert_eq!(failed("make", &third, false), None);
        let second = format!("work\nexit code 7\none\n{note}\ntwo");
        assert_eq!(
            failed("make", &second, false).map(|failure| failure.1),
            Some(Some(7))
        );
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
                evidence: Evidence::Marked,
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
                evidence: Evidence::Marked,
                exit_code: None,
                path: None,
                error: None,
                excerpt: Excerpt::Unscanned,
            })
        );
        // An unmarked result whose one ground is that unscanned code
        // changes no record: it is no failure and no pass.
        assert_eq!(
            input("make", &block, false).observe(SCOPE).err(),
            Some(None)
        );
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
        let reported = outcome_of(&command, &line, false);
        assert_eq!(reported.printed.as_ref().map(|(code, _)| *code), Some(7));
        // The digits are the one ground of the failure: the result changes
        // no record, and is no pass.
        assert_eq!(
            input(&command, &line, false).observe(SCOPE).err(),
            Some(None)
        );
        // A printed zero in such a line is no pass either, and a zero in a
        // line without a finding is one.
        let zero = format!("{key}=fixture-v0");
        assert!(kernel::exact_text_refusal(&zero).is_some());
        assert_eq!(
            input(&command, &zero, false).observe(SCOPE).err(),
            Some(None)
        );
        assert_eq!(said("deploy; echo \"rc=$?\"", "rc=0"), Said::Pass);
        // A marked result stays a failure and keeps no text of the line.
        let marked = recorded(&command, &line, true).unwrap();
        assert_eq!(
            (marked.evidence, marked.exit_code, &marked.excerpt),
            (Evidence::Marked, None, &Excerpt::Finding)
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
            input.observe(SCOPE).map_err(|reason| reason.is_some())
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
                Evidence::Marked,
                Some(101),
                "the command failed with exit code 101",
            ),
            (
                Evidence::Marked,
                None,
                "the command failed and the tool reported an error",
            ),
            (
                Evidence::Printed,
                Some(2),
                "the command failed: its command line printed exit status 2",
            ),
            (
                Evidence::Stated,
                Some(3),
                "the command failed: a status line of its output states exit code 3",
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
        let failure = failure(Evidence::Marked, Some(1), Excerpt::Kept(lines));
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
            evidence: Evidence::Stated,
            exit_code: Some(i64::MAX),
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
                        evidence: Evidence::Marked,
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
            let failure = failure(Evidence::Marked, Some(1), Excerpt::Kept(Vec::new()));
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

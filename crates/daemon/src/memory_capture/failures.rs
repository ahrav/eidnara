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

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use memory_store::{BlockKind, OutputKind, ResultBlockKind};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::native::{completes_failure_line, is_output_failure_line, printable};
use super::{CAPTURE_SOURCE_KIND, COMMIT_WAIT, capture_enabled};
use crate::harness_sources::Harness;
use crate::healing::SerializerProfile;
use crate::kernel_routes::{self, ProjectBinding};
use crate::wire::IngressMessage;
use crate::{HandlerCore, SessionBinding, now_ms};

/// The source lineage of every failure record.
pub(crate) const FAILURE_SOURCE_ID: &str = "tool_failure";
/// The category of a failure record. The block renders positive categories,
/// and this is none of them, so a record is served on explicit search alone.
pub(crate) const FAILURE_CATEGORY: &str = "TOOL_FAILURE";

/// Lines that state an error kept from one result, in output order. A
/// compiler or a test runner lists several, and the first ones name the
/// failure.
const MAX_ERROR_LINES: usize = 5;
/// Closing lines kept from one result: a failing command states its outcome
/// last.
const MAX_CLOSING_LINES: usize = 2;
/// One kept line, in bytes, with the mark that ends a cut line.
const MAX_LINE_BYTES: usize = 160;
/// The command as the record's text shows it, in bytes.
const MAX_COMMAND_BYTES: usize = 300;
/// The key of a command's live record, in bytes.
const MAX_KEY_BYTES: usize = 512;
/// The tool's name as the record's text shows it, in bytes.
const MAX_TOOL_BYTES: usize = 64;
/// The record's text, in bytes. The search tool renders 1,024 bytes of a
/// fact's content, so the text and the first fields of the rationale fit.
const MAX_SUMMARY_BYTES: usize = 900;
/// The bytes of one result read for its outcome, taken from its end: the
/// exit code and the summary of a failing run stand in the closing lines.
const MAX_RESULT_BYTES: usize = 64 * 1024;
/// The first line of a result is read for an exit code within these bytes.
const MAX_FIRST_LINE_BYTES: usize = 1024;
/// Closing lines read for a stated exit code.
const EXIT_CODE_CLOSING_LINES: usize = 3;
/// Records one session writes. A session that fails more commands than this
/// keeps the records it has.
const MAX_RECORDS_PER_SESSION: usize = 64;
/// Each pass retains at most 256 pending command results, favoring later
/// entries in `request.messages`.
const MAX_RESULTS_PER_PASS: usize = 256;
/// Tool calls one session's memo remembers.
const MEMO_CALLS_PER_SESSION: usize = 4096;
const MEMO_SESSIONS: usize = 256;
/// Failure records one Kernel read loads while a project's live records are
/// rebuilt.
const HEAD_PAGE_ROWS: usize = 256;
/// Pages one rebuild of a project's live records reads.
const MAX_HEAD_PAGES: usize = 64;
/// Marks a line, a command, or a key cut to its byte limit.
const CUT_MARK: &str = "…";
/// The sentence a resolved record's first line ends with.
const RESOLVED_NOTE: &str = " Resolved: a later run of this command reported no failure.";
/// What stands in place of a credential the scanner has no rule for.
const CREDENTIAL_MARK: &str = "<REDACTED:credential>";

/// What one tool result says about its command.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// The result reports no failure.
    Passed,
    Failed(Failure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Failure {
    /// The nonzero exit code the harness reports or the result states.
    exit_code: Option<i64>,
    /// The harness marked the result as an error.
    flagged: bool,
    /// The lines that state an error and the closing lines, in output order.
    lines: Vec<Kept>,
}

/// One line kept from a failed result, redacted and within
/// [`MAX_LINE_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Kept {
    text: String,
    /// The line is one of the output's closing lines.
    closing: bool,
}

/// The run a record is ordered by.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Run {
    at_ms: i64,
    /// The harness gave the result's creation time. Without one, `at_ms` is
    /// the time of the first judgment.
    timed: bool,
    session: String,
    /// The result message's position in its request's window, from 1.
    place: u64,
}

impl Run {
    /// Whether this run is later than `earlier`. Two runs of one session
    /// order by place when either lacks a creation time or the times are
    /// equal. Runs of two sessions order by time, and equal times leave the
    /// earlier record in place.
    fn follows(&self, earlier: &Run) -> bool {
        let one_session = self.session == earlier.session;
        if one_session && !(self.timed && earlier.timed) {
            return self.place > earlier.place;
        }
        self.at_ms > earlier.at_ms
            || (one_session && self.at_ms == earlier.at_ms && self.place > earlier.place)
    }
}

/// One command result of the window that the session's memo has not seen:
/// the messages that hold the call and the result, shared with the pass.
struct Pending {
    call: Arc<IngressMessage>,
    call_block: usize,
    result: Arc<IngressMessage>,
    result_block: usize,
    /// The OpenCode row of the result's message, which holds the exit status
    /// the harness reports.
    native: Option<Arc<Value>>,
    place: u64,
}

/// One command result as the detached task read it.
struct Observed {
    call: String,
    tool: String,
    /// The command as the record's text shows it: redacted, with whitespace
    /// runs as single spaces.
    shown: String,
    key: String,
    run: Run,
    outcome: Outcome,
}

/// The live record of one command.
#[derive(Clone)]
struct Head {
    id: String,
    revision: i64,
    resolved: bool,
    /// The failed run, or the passing run once the record is resolved.
    run: Run,
    summary: String,
    rationale: serde_json::Map<String, Value>,
}

/// The tool calls of one session that its checkpoints have judged.
#[derive(Default)]
struct SessionCalls {
    /// Distinguishes this entry from one created after the session's delete.
    epoch: u64,
    /// A checkpoint of the session is admitted and has not finished. The
    /// entry stays in the memo while it is set.
    in_flight: bool,
    calls: HashSet<String>,
}

/// The live records of one project and the number each session holds among
/// them, as the Kernel serves them.
#[derive(Default)]
struct ProjectRecords {
    heads: HashMap<String, Head>,
    held: HashMap<String, usize>,
}

/// Tool calls already judged, by session, and the live record of each
/// command, by project. Both are rebuilt from the window and the Kernel after
/// a restart: a record's id derives from its tool call, so a call judged
/// again writes nothing.
#[derive(Default)]
pub(crate) struct FailureMemo {
    sessions: HashMap<String, SessionCalls>,
    /// Sessions in the order their entries were created.
    order: VecDeque<String>,
    next_epoch: u64,
    projects: HashMap<String, ProjectRecords>,
    /// Projects whose rebuild passed [`MAX_HEAD_PAGES`] and was reported.
    oversized: HashSet<String>,
}

impl FailureMemo {
    fn has_seen(&self, session: &str, call: &str) -> bool {
        self.sessions
            .get(session)
            .is_some_and(|entry| entry.calls.contains(call))
    }

    fn in_flight(&self, session: &str) -> bool {
        self.sessions
            .get(session)
            .is_some_and(|entry| entry.in_flight)
    }

    /// Marks the session's entry as held by one checkpoint and returns its
    /// epoch, creating the entry when absent. A new entry at the session
    /// limit takes the place of the oldest entry no checkpoint holds.
    fn pin(&mut self, session: &str) -> u64 {
        if !self.sessions.contains_key(session) {
            while self.sessions.len() >= MEMO_SESSIONS {
                let victim = self
                    .order
                    .iter()
                    .position(|kept| self.sessions.get(kept).is_none_or(|entry| !entry.in_flight));
                match victim.and_then(|index| self.order.remove(index)) {
                    Some(victim) => {
                        self.sessions.remove(&victim);
                    }
                    None => break,
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
        let entry = self
            .sessions
            .get_mut(session)
            .expect("entry was just ensured");
        entry.in_flight = true;
        entry.epoch
    }

    fn release(&mut self, session: &str, epoch: u64) {
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

    /// Remembers a judged call. A full entry keeps the calls it holds, and
    /// the Kernel's record of a call answers for the rest.
    fn learn(&mut self, session: &str, epoch: u64, call: &str) {
        if let Some(entry) = self
            .sessions
            .get_mut(session)
            .filter(|entry| entry.epoch == epoch)
            && entry.calls.len() < MEMO_CALLS_PER_SESSION
        {
            entry.calls.insert(call.to_owned());
        }
    }

    pub(crate) fn forget_session(&mut self, session: &str) {
        if self.sessions.remove(session).is_some() {
            self.order.retain(|kept| kept != session);
        }
    }

    #[cfg(test)]
    pub(crate) fn has_seen_for_test(&self, session: &str, call: &str) -> bool {
        self.has_seen(session, call)
    }

    #[cfg(test)]
    pub(crate) fn calls_for_test(&self, session: &str) -> usize {
        self.sessions
            .get(session)
            .map_or(0, |entry| entry.calls.len())
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
/// command line, else `cmd`. A shell tool of any harness and any name
/// carries one.
fn command_field(input: &Value) -> Option<&Value> {
    ["command", "cmd"]
        .into_iter()
        .filter_map(|name| input.get(name))
        .find(|field| carries_command(field))
}

/// The command line a tool call carries, with an array of strings joined by
/// spaces.
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

/// The end of the shell word `text` starts with: the first whitespace, `;`,
/// `&`, or `|` outside quotes and backslash escapes. `None` when a quote
/// stays open.
fn word_end(text: &str) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => {}
            (_, '\\') => escaped = true,
            (None, '\'' | '"') => quote = Some(ch),
            (None, _) if ch.is_whitespace() || matches!(ch, ';' | '&' | '|') => {
                return Some(index);
            }
            _ => {}
        }
    }
    quote.is_none().then_some(text.len())
}

/// What follows a leading `cd <path> &&` or `cd <path>;` step, where the
/// path is one shell word.
fn after_cd(text: &str) -> Option<&str> {
    let path = text.strip_prefix("cd")?;
    if !path.starts_with(char::is_whitespace) {
        return None;
    }
    let path = path.trim_start();
    let end = word_end(path).filter(|end| *end > 0)?;
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
    let after = &value[word_end(value)?..];
    after
        .starts_with(char::is_whitespace)
        .then(|| after.trim_start())
}

/// The command without what varies between runs of one operation: leading
/// `cd <path> &&` and `cd <path>;` steps, leading `NAME=value` assignments,
/// and the width of whitespace runs. A step or an assignment that ends the
/// command stays. Each step is read once, so the work is linear in the
/// command's bytes.
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

/// Names of flags and assignments whose value is a credential, matched
/// inside the name in any letter case.
const SECRET_NAMES: [&str; 7] = [
    "password", "passwd", "token", "secret", "api-key", "apikey", "api_key",
];

fn names_secret(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET_NAMES.iter().any(|secret| name.contains(secret))
}

/// `text` with the userinfo of each URL replaced by [`CREDENTIAL_MARK`]:
/// `scheme://user:secret@host` keeps `scheme://` and `@host`. The userinfo
/// ends at the last `@` before the first `/`, `?`, `#`, or whitespace.
fn mask_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (before, after) = rest.split_at(at + 3);
        out.push_str(before);
        let authority = after
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '/' | '?' | '#'))
            .unwrap_or(after.len());
        let scheme = before[..at].ends_with(|ch: char| ch.is_ascii_alphanumeric());
        rest = match after[..authority].rfind('@') {
            Some(user) if scheme && user > 0 => {
                out.push_str(CREDENTIAL_MARK);
                &after[user..]
            }
            _ => after,
        };
    }
    out.push_str(rest);
    out
}

/// `text` with [`CREDENTIAL_MARK`] in place of the value of each flag or
/// assignment whose name is one of [`SECRET_NAMES`]: `--name value`,
/// `--name=value`, and `NAME=value`. A quoted value runs to its closing
/// quote on the same line.
fn mask_named_values(text: &str) -> String {
    enum Part {
        /// The words read are no part of a value.
        Outside,
        /// The next word of the line is the value of a flag.
        Next,
        /// The words through the closing quote are the rest of a value.
        Quoted(char),
    }
    let quoted = |value: &str| match value.chars().next() {
        Some(quote @ ('"' | '\'')) if value.len() == 1 || !value.ends_with(quote) => {
            Part::Quoted(quote)
        }
        _ => Part::Outside,
    };
    let mut out = String::with_capacity(text.len());
    let mut state = Part::Outside;
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end_matches(char::is_whitespace);
        let in_value = matches!(state, Part::Quoted(_));
        state = match state {
            _ if word.is_empty() => state,
            Part::Quoted(quote) if word.ends_with(quote) => Part::Outside,
            Part::Quoted(quote) => Part::Quoted(quote),
            Part::Next if !word.starts_with('-') => {
                out.push_str(CREDENTIAL_MARK);
                quoted(word)
            }
            _ => match word.split_once('=') {
                Some((name, value)) if names_secret(name) && !value.is_empty() => {
                    out.push_str(name);
                    out.push('=');
                    out.push_str(CREDENTIAL_MARK);
                    quoted(value)
                }
                found => {
                    out.push_str(word);
                    if found.is_none() && word.starts_with('-') && names_secret(word) {
                        Part::Next
                    } else {
                        Part::Outside
                    }
                }
            },
        };
        let space = &piece[word.len()..];
        if !in_value || space.contains('\n') {
            out.push_str(space);
        }
        if space.contains('\n') {
            state = Part::Outside;
        }
    }
    out
}

/// `text` as a record stores it: URL userinfo and the values of
/// secret-named flags and assignments masked, then redacted as capture's
/// sources are. `None` when the scanner fails and its placeholder stands for
/// the whole text.
fn redacted(text: &str) -> Option<String> {
    let masked = mask_named_values(&mask_userinfo(text));
    let redaction = context_core::redaction::redact_durable_text(&masked);
    let whole = redaction.text == "<REDACTED:secret>"
        && redaction.text != masked
        && matches!(redaction.detections.as_slice(), [all] if all.length == masked.len());
    (!whole).then_some(redaction.text)
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

/// The key of a command's live record: the redacted normalized command. A
/// command longer than [`MAX_KEY_BYTES`] is keyed by its first bytes and its
/// byte length. The key is plain text, so the Kernel's redaction of stored
/// fields leaves it as written and a rebuilt memo reads back the key it
/// wrote.
fn command_key(normalized: &str) -> String {
    if normalized.len() <= MAX_KEY_BYTES {
        return normalized.to_owned();
    }
    let length = format!("{CUT_MARK} [{} bytes]", normalized.len());
    let keep = normalized.floor_char_boundary(MAX_KEY_BYTES - length.len());
    format!("{}{length}", &normalized[..keep])
}

/// The words a shell tool prints before an exit code.
const EXIT_PHRASES: [&str; 4] = ["exit code", "exit_code", "exited with code", "exit status"];

/// Fields of a structured result whose integer is the command's exit code.
const EXIT_FIELDS: [&str; 5] = ["exit_code", "exitCode", "exit", "status_code", "code"];

/// The exit code a line states: digits after one of [`EXIT_PHRASES`], in any
/// letter case, past a `:`, `=`, `"`, or spaces. The last phrase of the line
/// that digits follow states the code.
fn stated_exit_code(line: &str) -> Option<i64> {
    let lower = line.to_ascii_lowercase();
    EXIT_PHRASES
        .iter()
        .flat_map(|phrase| {
            lower
                .match_indices(phrase)
                .map(move |(at, _)| at + phrase.len())
        })
        .filter_map(|after| {
            let digits = lower[after..].trim_start_matches([':', '=', ' ', '"']);
            let end = digits
                .find(|ch: char| !ch.is_ascii_digit())
                .unwrap_or(digits.len());
            Some((after, digits[..end].parse().ok()?))
        })
        .max_by_key(|(after, _)| *after)
        .map(|(_, code)| code)
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

/// A tool result as its outcome is read.
struct Reading {
    /// The first non-empty line of the output, within
    /// [`MAX_FIRST_LINE_BYTES`]. It is read for an exit code and kept in no
    /// record.
    first: Option<String>,
    /// The last [`MAX_RESULT_BYTES`] of the output, from the start of a line.
    window: String,
    /// The harness marked the result as an error.
    flagged: bool,
    /// The exit code a field of a structured result holds.
    field_exit: Option<i64>,
}

impl Reading {
    /// Reads `parts` as one output with a line break between parts. The
    /// bytes copied stay within [`MAX_RESULT_BYTES`].
    fn of_parts(parts: &[&str], flagged: bool, field_exit: Option<i64>) -> Self {
        let first = parts
            .iter()
            .flat_map(|part| part.lines())
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| line[..line.floor_char_boundary(MAX_FIRST_LINE_BYTES)].to_owned());
        let mut budget = MAX_RESULT_BYTES;
        let mut kept = Vec::new();
        let mut cut = false;
        for part in parts.iter().rev() {
            if part.len() > budget {
                kept.push(&part[part.ceil_char_boundary(part.len() - budget)..]);
                cut = true;
                break;
            }
            kept.push(part);
            budget = budget.saturating_sub(part.len() + 1);
        }
        kept.reverse();
        let mut window = kept.join("\n");
        if cut {
            // The cut can split a line, and the scanner reads a split secret
            // as plain text, so the window starts at its first whole line.
            window = window
                .split_once('\n')
                .map_or_else(String::new, |(_, whole)| whole.to_owned());
        }
        Self {
            first,
            window,
            flagged,
            field_exit,
        }
    }

    /// Text that is one JSON object within [`MAX_RESULT_BYTES`] is read by
    /// its fields, as a structured result is.
    fn of_text(text: &str, flagged: bool) -> Self {
        if text.len() <= MAX_RESULT_BYTES
            && text.trim_start().starts_with('{')
            && let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(text)
        {
            return Self::of_value(&value, flagged);
        }
        Self::of_parts(&[text], flagged, None)
    }

    /// A string is read as text and an object by its fields. Any other value
    /// is read as its JSON text.
    fn of_value(value: &Value, flagged: bool) -> Self {
        match value {
            Value::String(text) => Self::of_text(text, flagged),
            Value::Object(fields) => {
                let mut parts = Vec::new();
                let mut exit = None;
                read_fields(fields, 1, &mut parts, &mut exit);
                Self::of_parts(&parts, flagged, exit)
            }
            other => Self::of_parts(&[&other.to_string()], flagged, None),
        }
    }

    /// `None` for a result the harness denied: the command did not run.
    fn of_output(output: &OutputKind) -> Option<Self> {
        let blocks = |blocks: &[memory_store::ResultBlock], flagged| {
            let parts: Vec<&str> = blocks
                .iter()
                .filter_map(|block| match &block.kind {
                    ResultBlockKind::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            Self::of_parts(&parts, flagged, None)
        };
        Some(match output {
            OutputKind::Text { text } => Self::of_text(text, false),
            OutputKind::ErrorText { text } => Self::of_text(text, true),
            OutputKind::Content { blocks: content } => blocks(content, false),
            OutputKind::ErrorContent { blocks: content } => blocks(content, true),
            OutputKind::Json { value } => Self::of_value(value, false),
            OutputKind::ErrorJson { value } => Self::of_value(value, true),
            OutputKind::ExecutionDenied { .. } => return None,
        })
    }
}

/// Collects what a structured result states, to two levels of nesting: its
/// string values in field order, and its exit code, the first integer under
/// one of [`EXIT_FIELDS`], an object's own fields before those it nests.
/// Every other number and every boolean is left out.
fn read_fields<'a>(
    fields: &'a serde_json::Map<String, Value>,
    depth: usize,
    parts: &mut Vec<&'a str>,
    exit: &mut Option<i64>,
) {
    if exit.is_none() {
        *exit = EXIT_FIELDS
            .iter()
            .find_map(|name| fields.get(*name)?.as_i64());
    }
    for value in fields.values() {
        match value {
            Value::String(text) => parts.push(text),
            Value::Object(nested) if depth < 2 => read_fields(nested, depth + 1, parts, exit),
            _ => {}
        }
    }
}

/// What a command's result reports. `reading` holds the redacted output and
/// `native_exit` the exit status the harness reports. The harness's error
/// mark decides a failure. Otherwise an exit code decides: zero is a pass and
/// any other code a failure. The code is the harness's, else a structured
/// result's field, else the last one stated in the first line and the last
/// [`EXIT_CODE_CLOSING_LINES`] lines of the output. Without a code, the
/// result is a failure when a line is a failure line as capture reads one in
/// an output block, or when the shell reports a missing command. The kept
/// lines are the first lines that state an error, each failure line with the
/// line that completes it, and then the closing lines of the output.
fn outcome_of(reading: &Reading, native_exit: Option<i64>) -> Outcome {
    let lines: Vec<&str> = reading.window.lines().map(str::trim).collect();
    let shown = |line: &str| !line.is_empty() && printable(line);
    let stated = || {
        lines
            .iter()
            .rev()
            .filter(|line| !line.is_empty())
            .take(EXIT_CODE_CLOSING_LINES)
            .find_map(|line| stated_exit_code(line))
            .or_else(|| reading.first.as_deref().and_then(stated_exit_code))
    };
    let code = native_exit.or(reading.field_exit).or_else(stated);
    let mut fails = false;
    let mut errors: Vec<&str> = Vec::new();
    for (index, line) in lines.iter().enumerate().filter(|(_, line)| shown(line)) {
        let failure = is_output_failure_line(line);
        fails |= failure || line.contains(MISSING_COMMAND);
        let completing = index > 0
            && is_output_failure_line(lines[index - 1])
            && completes_failure_line(lines[index - 1], line);
        if errors.len() < MAX_ERROR_LINES && (failure || completing || states_error(line)) {
            errors.push(line);
        }
    }
    if !(reading.flagged || code.map_or(fails, |code| code != 0)) {
        return Outcome::Passed;
    }
    let mut closing: Vec<&str> = lines
        .iter()
        .rev()
        .copied()
        .filter(|line| shown(line))
        .take(MAX_CLOSING_LINES)
        .collect();
    closing.reverse();
    let mut kept: Vec<Kept> = Vec::new();
    let lines = errors
        .into_iter()
        .map(|line| (line, false))
        .chain(closing.into_iter().map(|line| (line, true)));
    for (line, closing) in lines {
        let text = clipped(line, MAX_LINE_BYTES);
        // A closing line that an error line already holds marks that line.
        match kept.iter_mut().find(|kept| kept.text == text) {
            Some(earlier) => earlier.closing |= closing,
            None => kept.push(Kept { text, closing }),
        }
    }
    Outcome::Failed(Failure {
        exit_code: code.filter(|code| *code != 0),
        flagged: reading.flagged,
        lines: kept,
    })
}

/// The record's text within [`MAX_SUMMARY_BYTES`], with room for
/// [`RESOLVED_NOTE`]: what the result states, the command, and the kept
/// lines. The closing lines are kept whole, and the lines that state an
/// error fill the bytes that remain.
fn summary_of(tool: &str, shown: &str, failure: &Failure) -> String {
    let stated = match (failure.exit_code, failure.flagged) {
        (Some(code), _) => format!("the command failed with exit code {code}"),
        (None, true) => "the command failed and the tool reported an error".to_owned(),
        (None, false) => {
            "the command failed by the failure lines of its output, which states no exit code"
                .to_owned()
        }
    };
    let tool = clipped(&collapsed(tool), MAX_TOOL_BYTES);
    let command = clipped(shown, MAX_COMMAND_BYTES);
    let mut summary = format!(
        "Tool failure recorded from the result of {tool}: {stated}.\nCommand: {command}\nOutput lines:"
    );
    let limit = MAX_SUMMARY_BYTES - RESOLVED_NOTE.len();
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
    // The parts are redacted. The scan of the joined text covers the tool's
    // name and a secret that only the joined text shows.
    clipped(
        &context_core::redaction::redact_durable_text(&summary).text,
        limit,
    )
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
}

/// Holds a session's memo entry for one checkpoint; dropping it, on any
/// path, admits the session's next checkpoint.
struct MemoPin {
    memo: Arc<Mutex<FailureMemo>>,
    session: String,
    epoch: u64,
}

impl Drop for MemoPin {
    fn drop(&mut self) {
        self.memo
            .lock()
            .expect("failure memo mutex")
            .release(&self.session, self.epoch);
    }
}

/// The id of an OpenCode row, which is the id of the message derived from
/// it.
fn native_row_id(row: &Value) -> Option<&str> {
    row.get("info")
        .and_then(|info| info.get("id"))
        .or_else(|| row.get("id"))
        .and_then(Value::as_str)
}

/// The exit status OpenCode reports for the tool call `call`, read from the
/// row of the call's message.
fn reported_exit(row: &Value, call: &str) -> Option<i64> {
    let part = row
        .get("parts")?
        .as_array()?
        .iter()
        .find(|part| part.get("callID").and_then(Value::as_str) == Some(call))?;
    crate::tool_vocabulary::exit_code(
        Harness::OpenCode,
        part.get("tool")?.as_str()?,
        part.get("state")?.get("metadata"),
    )
}

impl HandlerCore {
    /// References to the command results of the window that this session's
    /// memo has not judged, when capture is on for a primary session. The
    /// reply path walks the window's blocks and copies no output: the
    /// detached task reads the text. One checkpoint of a session is in
    /// flight at a time and holds at most [`MAX_RESULTS_PER_PASS`] results,
    /// so the windows that detached tasks keep alive stay bounded. Returns
    /// `None` when the window holds no unseen result, so the caller spawns no
    /// task.
    pub(crate) fn failure_checkpoint(
        &self,
        binding: &SessionBinding,
        request: &crate::transform::TransformRequest,
    ) -> Option<FailureCheckpoint> {
        if request.is_subagent || !capture_enabled(binding) {
            return None;
        }
        let mut memo = self.failure_memo.lock().expect("failure memo mutex");
        if memo.in_flight(&binding.session) {
            return None;
        }
        // An OpenCode window carries the rows its messages derive from, in
        // window order.
        let mut rows = request
            .native_messages
            .as_deref()
            .filter(|_| request.serializer_profile == SerializerProfile::OpencodeAiSdk.wire_id())
            .unwrap_or_default()
            .iter();
        let mut row: Option<(usize, Option<Arc<Value>>)> = None;
        let mut calls: HashMap<&str, (usize, usize)> = HashMap::new();
        let mut pending = VecDeque::new();
        for (index, message) in request.messages.iter().enumerate() {
            for (block_index, block) in message.ck.content().iter().enumerate() {
                match block.kind() {
                    BlockKind::ToolCall { id, input, .. } => {
                        if command_field(input).is_some() {
                            calls.insert(id.as_str(), (index, block_index));
                        }
                    }
                    BlockKind::ToolResult { id, .. } => {
                        let Some(&(call, call_block)) = calls.get(id.as_str()) else {
                            continue;
                        };
                        if memo.has_seen(&binding.session, id) {
                            continue;
                        }
                        if row.as_ref().is_none_or(|(of, _)| *of != index) {
                            let found = rows
                                .find(|row| native_row_id(row) == Some(message.mid.as_str()))
                                .cloned();
                            row = Some((index, found));
                        }
                        pending.push_back(Pending {
                            call: Arc::clone(&request.messages[call]),
                            call_block,
                            result: Arc::clone(message),
                            result_block: block_index,
                            native: row.as_ref().and_then(|(_, found)| found.clone()),
                            place: index as u64 + 1,
                        });
                        if pending.len() > MAX_RESULTS_PER_PASS {
                            pending.pop_front();
                        }
                    }
                    _ => {}
                }
            }
        }
        if pending.is_empty() {
            return None;
        }
        let epoch = memo.pin(&binding.session);
        Some(FailureCheckpoint {
            kernel: Arc::clone(&self.kernel),
            commit_gate: Arc::clone(&self.capture_commit_gate),
            project: binding.kernel_project.clone(),
            pin: MemoPin {
                memo: Arc::clone(&self.failure_memo),
                session: binding.session.clone(),
                epoch,
            },
            pending: pending.into(),
        })
    }
}

impl Pending {
    /// The id of the tool call the result answers.
    fn call_id(&self) -> Option<&str> {
        match self.result.ck.content().get(self.result_block)?.kind() {
            BlockKind::ToolResult { id, .. } => Some(id),
            _ => None,
        }
    }

    /// Reads the command and the result's outcome. The output's window and
    /// the command are redacted whole before any line is chosen or cut.
    /// `None` for a result the harness denied and for text the scanner
    /// fails on: the record would hold a placeholder.
    fn observe(&self, session: &str) -> Option<Observed> {
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
        let command = command_of(input)?;
        let mut reading = Reading::of_output(&output.kind)?;
        reading.window = redacted(&reading.window)?;
        let created = self.result.ck.meta.created_at_ms;
        Some(Observed {
            call: id.clone(),
            tool: name.clone(),
            shown: redacted(&collapsed(&command))?,
            key: command_key(&redacted(&normalized(&command))?),
            run: Run {
                at_ms: created.unwrap_or_else(now_ms),
                timed: created.is_some(),
                session: session.to_owned(),
                place: self.place,
            },
            outcome: outcome_of(
                &reading,
                self.native
                    .as_deref()
                    .and_then(|row| reported_exit(row, id)),
            ),
        })
    }
}

impl FailureCheckpoint {
    /// Text scans, Kernel reads, and commits run on the blocking pool.
    /// Returns the number of rows written.
    pub(crate) async fn run(self) -> usize {
        kernel_routes::blocking(move || self.record())
            .await
            .unwrap_or(0)
    }

    fn record(self) -> usize {
        let Self {
            kernel,
            commit_gate,
            project,
            pin,
            pending,
        } = self;
        let Ok(kernel) = kernel.kernel_store() else {
            return 0;
        };
        let writer = Writer {
            kernel: &kernel,
            project: &project,
            memo: &pin.memo,
            scope: project.digest(),
            session: &pin.session,
        };
        let mut written = 0;
        let mut refused = false;
        for result in &pending {
            let Some(call) = result.call_id() else {
                continue;
            };
            let unit = result.observe(&pin.session);
            // One gate orders the head read, the commit, and the memo update
            // of every checkpoint, and session deletion takes it too.
            let _gate = commit_gate.lock().expect("capture publication mutex");
            let memo = || pin.memo.lock().expect("failure memo mutex");
            if !memo().current(&pin.session, pin.epoch) {
                break;
            }
            let judged = match &unit {
                Some(unit) => writer.judge(unit),
                None => Ok(Judged::Nothing),
            };
            match judged {
                Ok(Judged::Deferred) => break,
                Ok(judged) => {
                    written += usize::from(judged == Judged::Wrote);
                    memo().learn(&pin.session, pin.epoch, call);
                }
                // The memo has not learned the call, so the next pass judges
                // it again.
                Err(error) => {
                    eprintln!("daemon: tool failure record deferred: {error}");
                    let busy = matches!(
                        error,
                        kernel::KernelError::Deadline
                            | kernel::KernelError::Busy
                            | kernel::KernelError::Held
                    );
                    if !busy {
                        // The cached live records can name a row another
                        // writer retired, so the next judgment reads them
                        // from the Kernel.
                        memo().projects.remove(writer.scope);
                    }
                    if busy || refused {
                        break;
                    }
                    refused = true;
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

/// Writes a run into a record's rationale: the failed run under the bare
/// names, the resolving run under `resolved_` names.
fn write_run(rationale: &mut serde_json::Map<String, Value>, prefix: &str, run: &Run) {
    let source = if run.timed { "result" } else { "judged" };
    for (name, value) in [
        ("session", json!(run.session)),
        ("at_ms", json!(run.at_ms)),
        ("at", json!(utc_time(run.at_ms))),
        ("at_source", json!(source)),
        ("place", json!(run.place)),
    ] {
        rationale.insert(format!("{prefix}{name}"), value);
    }
}

/// The run [`write_run`] wrote under `prefix`.
fn read_run(rationale: &serde_json::Map<String, Value>, prefix: &str) -> Run {
    let field = |name: &str| rationale.get(&format!("{prefix}{name}"));
    Run {
        at_ms: field("at_ms").and_then(Value::as_i64).unwrap_or(0),
        timed: field("at_source").and_then(Value::as_str) == Some("result"),
        session: field("session")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        place: field("place").and_then(Value::as_u64).unwrap_or(0),
    }
}

/// A tool call's id as a rationale shows it, in bytes.
const MAX_CALL_BYTES: usize = 128;

/// What judging one command result did.
#[derive(Debug, PartialEq, Eq)]
enum Judged {
    Wrote,
    Nothing,
    /// The project's live records are unread, so the result waits for a
    /// later pass.
    Deferred,
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
    /// Writes what one command result calls for. The caller holds the
    /// capture publication gate. A failure writes its record when it follows
    /// the command's live record and the session holds fewer than
    /// [`MAX_RECORDS_PER_SESSION`]. A pass rewrites an unresolved live
    /// record it follows as resolved. Either writes nothing when the Kernel
    /// holds the record of its tool call.
    fn judge(&self, unit: &Observed) -> Result<Judged, kernel::KernelError> {
        if !self.rebuilt()? {
            return Ok(Judged::Deferred);
        }
        let (head, held) = {
            let memo = self.memo.lock().expect("failure memo mutex");
            let records = memo.projects.get(self.scope);
            (
                records.and_then(|records| records.heads.get(&unit.key).cloned()),
                records
                    .and_then(|records| records.held.get(self.session).copied())
                    .unwrap_or(0),
            )
        };
        let follows = head.as_ref().is_none_or(|head| unit.run.follows(&head.run));
        let writes = follows
            && match &unit.outcome {
                Outcome::Passed => head.as_ref().is_some_and(|head| !head.resolved),
                Outcome::Failed(_) => held < MAX_RECORDS_PER_SESSION,
            };
        if !writes {
            return Ok(Judged::Nothing);
        }
        let id = record_id(self.scope, self.session, &unit.call);
        let (_, states) = self.kernel.object_states(std::slice::from_ref(&id))?;
        if states[0].is_some() {
            return Ok(Judged::Nothing);
        }
        let wrote = match (&unit.outcome, head) {
            (Outcome::Failed(failure), head) => self.fail(unit, failure, id, head)?,
            (Outcome::Passed, Some(head)) => self.resolve(unit, id, head)?,
            (Outcome::Passed, None) => false,
        };
        Ok(if wrote {
            Judged::Wrote
        } else {
            Judged::Nothing
        })
    }

    /// Whether the memo holds the project's live records, which this reads
    /// from the Kernel when the memo has none: the failure rows of the
    /// project at one snapshot, their payloads in pages of
    /// [`HEAD_PAGE_ROWS`]. A project with more rows than [`MAX_HEAD_PAGES`]
    /// pages hold reads `false`, reported on stderr once.
    fn rebuilt(&self) -> Result<bool, kernel::KernelError> {
        let memo = || self.memo.lock().expect("failure memo mutex");
        if memo().projects.contains_key(self.scope) {
            return Ok(true);
        }
        let (known_as_of, _, rows) = kernel_routes::read::domain_decisions_in_serving_order(
            self.kernel,
            self.project,
            kernel::Surface::ExplicitSearch,
            self.kernel.tip()?,
            "memory",
        )?;
        let ids: Vec<(String, i64)> = rows
            .into_iter()
            .filter(|row| is_failure_record(&row.object))
            .map(|row| (row.object.object_id, row.object.source_revision))
            .collect();
        if ids.len() > HEAD_PAGE_ROWS * MAX_HEAD_PAGES {
            if memo().oversized.insert(self.scope.to_owned()) {
                eprintln!(
                    "daemon: tool failure records deferred: the project's live failure records pass the rebuild limit"
                );
            }
            return Ok(false);
        }
        let mut records = ProjectRecords::default();
        for page in ids.chunks(HEAD_PAGE_ROWS) {
            let page_ids: Vec<String> = page.iter().map(|(id, _)| id.clone()).collect();
            let mut decisions: HashMap<String, kernel::DecisionRow> = self
                .kernel
                .decisions_for_objects_as_of(&page_ids, known_as_of)?
                .into_iter()
                .map(|decision| (decision.object_id.clone(), decision))
                .collect();
            for (id, revision) in page {
                let decision = decisions
                    .remove(id)
                    .ok_or(kernel::KernelError::CorruptCanonicalRow)?;
                let Ok(Value::Object(rationale)) =
                    serde_json::from_str(&decision.payload.rationale)
                else {
                    continue;
                };
                let Some(key) = rationale.get("command").and_then(Value::as_str) else {
                    continue;
                };
                let key = key.to_owned();
                let resolved = rationale.get("state").and_then(Value::as_str) == Some("resolved");
                *records
                    .held
                    .entry(read_run(&rationale, "").session)
                    .or_default() += 1;
                // Rows arrive newest first, so the newest row of a key is
                // its live record.
                records.heads.entry(key).or_insert(Head {
                    id: id.clone(),
                    revision: *revision,
                    resolved,
                    run: read_run(&rationale, if resolved { "resolved_" } else { "" }),
                    summary: decision.payload.summary,
                    rationale,
                });
            }
        }
        memo().projects.insert(self.scope.to_owned(), records);
        Ok(true)
    }

    /// Commits `head` as the live record of `key`, in place of `superseded`
    /// when one is given, as a labeled row. `counted` adds the row to the
    /// records its session holds.
    fn commit(
        &self,
        key: &str,
        superseded: Option<&Head>,
        head: Head,
        counted: bool,
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
        let receipt =
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
        let mut memo = self.memo.lock().expect("failure memo mutex");
        if receipt.replayed {
            // The Kernel answered from an earlier receipt, so the row's
            // state is the Kernel's to tell.
            memo.projects.remove(self.scope);
            return Ok(false);
        }
        let records = memo.projects.entry(self.scope.to_owned()).or_default();
        if counted {
            *records.held.entry(self.session.to_owned()).or_default() += 1;
        }
        records.heads.insert(key.to_owned(), head);
        Ok(true)
    }

    fn fail(
        &self,
        unit: &Observed,
        failure: &Failure,
        id: String,
        earlier: Option<Head>,
    ) -> Result<bool, kernel::KernelError> {
        let mut rationale = serde_json::Map::new();
        for (name, value) in [
            ("source", json!(FAILURE_SOURCE_ID)),
            ("call", json!(clipped(&unit.call, MAX_CALL_BYTES))),
            (
                "tool",
                json!(clipped(&collapsed(&unit.tool), MAX_TOOL_BYTES)),
            ),
            ("command", json!(unit.key)),
            ("state", json!("failed")),
        ] {
            rationale.insert(name.to_owned(), value);
        }
        write_run(&mut rationale, "", &unit.run);
        if let Some(code) = failure.exit_code {
            rationale.insert("exit_code".to_owned(), json!(code));
        }
        let head = Head {
            id,
            revision: earlier.as_ref().map_or(1, |earlier| earlier.revision + 1),
            resolved: false,
            run: unit.run.clone(),
            summary: summary_of(&unit.tool, &unit.shown, failure),
            rationale,
        };
        self.commit(&unit.key, earlier.as_ref(), head, true)
    }

    /// Rewrites the live record of a command as resolved: the same text with
    /// [`RESOLVED_NOTE`] at the end of its first line, and the passing run
    /// in the rationale. The record is ordered by the passing run from then
    /// on.
    fn resolve(
        &self,
        unit: &Observed,
        id: String,
        earlier: Head,
    ) -> Result<bool, kernel::KernelError> {
        let line = earlier.summary.find('\n').unwrap_or(earlier.summary.len());
        let (stated, rest) = earlier.summary.split_at(line);
        let mut rationale = earlier.rationale.clone();
        rationale.insert("state".to_owned(), json!("resolved"));
        rationale.insert(
            "resolved_call".to_owned(),
            json!(clipped(&unit.call, MAX_CALL_BYTES)),
        );
        write_run(&mut rationale, "resolved_", &unit.run);
        let head = Head {
            id,
            revision: earlier.revision + 1,
            resolved: true,
            run: unit.run.clone(),
            summary: clipped(&format!("{stated}{RESOLVED_NOTE}{rest}"), MAX_SUMMARY_BYTES),
            rationale,
        };
        self.commit(&unit.key, Some(&earlier), head, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The outcome of a text result, read as the detached task reads it.
    fn judged(text: &str, flagged: bool, native_exit: Option<i64>) -> Outcome {
        let output = if flagged {
            OutputKind::ErrorText { text: text.into() }
        } else {
            OutputKind::Text { text: text.into() }
        };
        judged_output(&output, native_exit)
    }

    fn judged_output(output: &OutputKind, native_exit: Option<i64>) -> Outcome {
        let mut reading = Reading::of_output(output).unwrap();
        reading.window = redacted(&reading.window).unwrap();
        outcome_of(&reading, native_exit)
    }

    /// The exit code and the kept lines of a failed text result.
    fn failed(text: &str, flagged: bool) -> Option<(Option<i64>, Vec<String>)> {
        match judged(text, flagged, None) {
            Outcome::Failed(failure) => Some((failure.exit_code, texts(&failure))),
            Outcome::Passed => None,
        }
    }

    fn texts(failure: &Failure) -> Vec<String> {
        failure.lines.iter().map(|line| line.text.clone()).collect()
    }

    fn key(command: &str) -> String {
        command_key(&redacted(&normalized(command)).unwrap())
    }

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
        assert_eq!(command_of(&json!({"command": ["npm", 7]})), None);
        // `cmd` carries the command when `command` holds none.
        for command in [json!(null), json!("  "), json!(7), json!(["npm", 7])] {
            assert_eq!(
                command_of(&json!({"command": command, "cmd": "make check"})).as_deref(),
                Some("make check"),
                "{command}"
            );
        }
        assert_eq!(
            command_of(&json!({"command": "cargo test", "cmd": "make check"})).as_deref(),
            Some("cargo test")
        );
        for (left, right) in [
            ("cd services/api && cargo test", "cargo   test"),
            ("RUST_LOG=debug CI=1 cargo test", "cargo test"),
            ("cd a && cd b && make check", "make check"),
            ("cd \"sample workspace\" && make check", "make check"),
            ("cd 'sample workspace'; make check", "make check"),
            ("cd sample\\ workspace&&make check", "make check"),
            ("NAME=\"two words\" make check", "make check"),
        ] {
            assert_eq!(key(left), key(right), "{left:?}");
        }
        assert_ne!(key("cargo test"), key("cargo test -p api"));
        // An assignment or a step that is the whole command is the command,
        // and so is a step whose quote stays open or that runs in the
        // background.
        for whole in ["FOO=bar", "cd api &&", "cd \"api && make", "cd api & make"] {
            assert_eq!(normalized(whole), whole);
        }
    }

    #[test]
    fn a_long_command_is_keyed_by_its_first_bytes_and_its_length() {
        let long = |tail: &str| format!("make {} {tail}", "check ".repeat(120));
        let (short_tail, long_tail) = (key(&long("api")), key(&long("api-gateway")));
        assert_ne!(short_tail, long_tail);
        assert_eq!(short_tail, key(&long("api")));
        for key in [&short_tail, &long_tail] {
            assert!(key.len() <= MAX_KEY_BYTES, "{}", key.len());
        }
        assert!(short_tail.ends_with(" bytes]"), "{short_tail}");
        // The Kernel stores the key as written: its scanner leaves it whole.
        assert_eq!(
            context_core::redaction::redact_durable_text(&short_tail).text,
            short_tail
        );
    }

    #[test]
    fn a_credential_is_masked_in_a_url_and_after_a_secret_named_flag_or_assignment() {
        for (text, stored) in [
            (
                "psql postgres://app:hunter2@db.internal/app -c 'select 1'",
                "psql postgres://<REDACTED:credential>@db.internal/app -c 'select 1'",
            ),
            (
                "curl https://example.com/a@b ftp://example.com",
                "curl https://example.com/a@b ftp://example.com",
            ),
            (
                "deploy --password hunter2 --region eu",
                "deploy --password <REDACTED:credential> --region eu",
            ),
            (
                "deploy --api-key \"two words\" --region eu",
                "deploy --api-key <REDACTED:credential> --region eu",
            ),
            (
                "deploy --no-password --region eu",
                "deploy --no-password --region eu",
            ),
            (
                "deploy --client-secret=hunter2 stage",
                "deploy --client-secret=<REDACTED:credential> stage",
            ),
            (
                "DEPLOY_TOKEN='two words' make deploy",
                "DEPLOY_TOKEN=<REDACTED:credential> make deploy",
            ),
            (
                "login --passwd\nhunter2 is the next line",
                "login --passwd\nhunter2 is the next line",
            ),
        ] {
            let masked = mask_named_values(&mask_userinfo(text));
            assert_eq!(masked, stored, "{text:?}");
            // The scanner and a second pass leave the masked text as it is.
            assert_eq!(redacted(&masked).as_deref(), Some(stored), "{text:?}");
        }
    }

    #[test]
    fn a_secret_is_redacted_before_a_line_or_the_window_is_cut() {
        let secret = "correct-horse-battery-staple-0123456789";
        // The cut of a kept line falls inside the secret.
        let line = format!("error: {} db_password={secret}", "x".repeat(125));
        let (_, lines) = failed(&line, false).unwrap();
        assert!(lines[0].len() <= MAX_LINE_BYTES, "{lines:?}");
        assert!(!lines[0].contains("correct"), "{lines:?}");
        // The cut of the window falls inside the secret: the window starts
        // at the next whole line.
        let tail = format!("\n{}\nerror: boom", "y".repeat(MAX_RESULT_BYTES - 22));
        let text = format!("db_password={secret}{tail}");
        let cut = text.len() - MAX_RESULT_BYTES;
        assert!(cut > "db_password=".len() && cut < text.len() - tail.len());
        let reading = Reading::of_text(&text, false);
        assert!(
            reading.window.starts_with("yyyy"),
            "{:?}",
            &reading.window[..20]
        );
        assert!(reading.window.ends_with("\nerror: boom"));
        // A window that is one cut line holds nothing.
        let reading = Reading::of_text(&"z".repeat(MAX_RESULT_BYTES + 1), false);
        assert_eq!(reading.window, "");
        // Blocks are read as one output, from its end.
        let reading = Reading::of_parts(&["first\nsecond", "third"], false, None);
        assert_eq!(reading.window, "first\nsecond\nthird");
        assert_eq!(reading.first.as_deref(), Some("first"));
    }

    #[test]
    fn a_result_fails_by_the_harness_mark_an_exit_code_or_its_failure_lines() {
        // A stated code decides, in the words shell tools print.
        for text in [
            "boom\n\nCommand exited with code 2",
            "Exit code 2\nboom",
            "boom\nProcess exited with code 2",
            "boom\nExit code: 2",
            "boom\nexit status 2",
            "Exit code 2\nboom\nmore\nmore\nmore\nmore",
        ] {
            assert_eq!(
                failed(text, false).map(|(code, _)| code),
                Some(Some(2)),
                "{text:?}"
            );
        }
        assert_eq!(failed("ERROR in the log\nExit code: 0", false), None);
        assert_eq!(failed("all good", false), None);
        // The last stated code decides.
        assert_eq!(failed("Exit code: 2\nretrying\nExit code: 0", false), None);
        assert_eq!(
            failed("Exit code: 0\nretrying\nExit code: 2", false).map(|(code, _)| code),
            Some(Some(2))
        );
        // On one line, the last phrase that digits follow states the code.
        for (line, code) in [
            ("exit code was unavailable; command exit code: 2", Some(2)),
            ("worker exit code 1, then exit status 3", Some(3)),
            ("exit status 3 of the worker, exit code unknown", Some(3)),
            ("the exit code is missing", None),
        ] {
            assert_eq!(stated_exit_code(line), code, "{line:?}");
        }
        // A code a child process printed in the middle of the output decides
        // nothing.
        assert_eq!(
            failed(
                "start\nchild exited with code 3\nstep\nstep\nstep\ndone",
                false
            ),
            None
        );
        // The harness mark decides a failure, also over a stated zero.
        assert_eq!(
            failed("permission denied", true),
            Some((None, vec!["permission denied".to_owned()]))
        );
        assert_eq!(
            judged("timed out\nExit code: 0", true, None),
            Outcome::Failed(Failure {
                exit_code: None,
                flagged: true,
                lines: vec![
                    Kept {
                        text: "timed out".into(),
                        closing: true
                    },
                    Kept {
                        text: "Exit code: 0".into(),
                        closing: true
                    },
                ],
            })
        );
        // The exit status the harness reports decides before the text.
        let missing = "cat: notes.md: No such file or directory";
        assert_eq!(judged(missing, false, None), Outcome::Passed);
        assert_eq!(
            judged(missing, false, Some(1)),
            Outcome::Failed(Failure {
                exit_code: Some(1),
                flagged: false,
                lines: vec![Kept {
                    text: missing.into(),
                    closing: true
                }],
            })
        );
        assert_eq!(
            judged("error: unused\nExit code: 3", false, Some(0)),
            Outcome::Passed
        );
        // Failure lines, with no code stated: a pipeline hides the code of
        // the command that failed.
        let (code, lines) = failed(
            "running 2 tests\ntest parses ... ok\ntest renders ... FAILED\n\nthread 'renders' panicked at src/render.rs:9:5:\nwidth is zero\ntest result: FAILED. 1 passed; 1 failed",
            false,
        )
        .unwrap();
        assert_eq!(code, None);
        assert_eq!(
            lines,
            [
                "test renders ... FAILED",
                "thread 'renders' panicked at src/render.rs:9:5:",
                "width is zero",
                "test result: FAILED. 1 passed; 1 failed",
            ]
        );
        // A long line is cut within the line limit.
        let long = format!("error: {}", "x".repeat(600));
        let (_, lines) = failed(&long, false).unwrap();
        assert_eq!(lines[0].len(), MAX_LINE_BYTES);
        assert!(lines[0].ends_with(CUT_MARK));
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
    fn a_line_states_an_error_by_a_level_word_or_a_type_name() {
        for line in [
            "OSError: [Errno 28] No space left on device",
            "raise IOError(path)",
            "KeyError: 'id'",
            "java.lang.IllegalStateException: closed",
            "build FAILED",
            "sh: 1: fd: command not found",
        ] {
            assert!(states_error(line), "{line:?}");
        }
        for line in ["3 passed", "terrors and mirrors", "M src/app.ts"] {
            assert!(!states_error(line), "{line:?}");
        }
    }

    #[test]
    fn a_failed_result_keeps_its_first_error_lines_and_its_closing_lines() {
        let (_, lines) = failed(
            "> tsc --noEmit\n\nsrc/app.ts(3,10): error TS2307: Cannot find module './render'.\nsrc/app.ts(9,5): error TS7006: Parameter 'event' implicitly has an 'any' type.\nM src/app.ts\n?? notes.txt\n\nCommand exited with code 2",
            true,
        )
        .unwrap();
        assert_eq!(
            lines,
            [
                "src/app.ts(3,10): error TS2307: Cannot find module './render'.",
                "src/app.ts(9,5): error TS7006: Parameter 'event' implicitly has an 'any' type.",
                "?? notes.txt",
                "Command exited with code 2",
            ]
        );
        // More error lines than are kept: the first ones and the closing lines.
        let many: String = (0..20)
            .map(|index| format!("FAILED tests/test_{index}.py::test_case\n"))
            .chain(["20 failed, 3 passed in 1.2s\n".to_owned()])
            .collect();
        let (_, lines) = failed(&many, false).unwrap();
        assert_eq!(lines.len(), MAX_ERROR_LINES + MAX_CLOSING_LINES);
        assert_eq!(lines[0], "FAILED tests/test_0.py::test_case");
        assert_eq!(lines[MAX_ERROR_LINES], "FAILED tests/test_19.py::test_case");
        assert_eq!(lines.last().unwrap(), "20 failed, 3 passed in 1.2s");
        // The next line completes a failure line. A blank line between them
        // ends the failure line, as it does in an output block.
        let (_, lines) = failed("ERROR migrate:\nrelation exists\none\ntwo\nthree", false).unwrap();
        assert_eq!(lines, ["ERROR migrate:", "relation exists", "two", "three"]);
        let (_, lines) =
            failed("ERROR migrate:\n\nrelation exists\none\ntwo\nthree", false).unwrap();
        assert_eq!(lines, ["ERROR migrate:", "two", "three"]);
    }

    #[test]
    fn the_closing_lines_stay_when_the_kept_lines_pass_the_record_s_bytes() {
        let long = |name: &str| format!("error: {name} {}", "x".repeat(MAX_LINE_BYTES));
        let errors: Vec<String> = (0..MAX_ERROR_LINES)
            .map(|index| long(&format!("e{index}")))
            .collect();
        let limit = MAX_SUMMARY_BYTES - RESOLVED_NOTE.len();
        let command = "make ".repeat(80);
        let summary = |output: &[String]| {
            let Outcome::Failed(failure) = judged(&output.join("\n"), true, None) else {
                panic!("the result failed");
            };
            let summary = summary_of("bash", command.trim(), &failure);
            assert!(summary.len() <= limit, "{}", summary.len());
            summary
        };
        // Two long closing lines after the error lines: the room that the
        // command leaves holds the two closing lines.
        let mut output = errors.clone();
        output.extend([long("c0"), long("c1")]);
        let kept = summary(&output);
        let closing = format!(
            "\n{}\n{}",
            clipped(&long("c0"), MAX_LINE_BYTES),
            clipped(&long("c1"), MAX_LINE_BYTES)
        );
        assert!(kept.ends_with(&closing), "{kept}");
        assert!(!kept.contains("error: e0"), "{kept}");
        // The closing lines repeat the first two error lines: those two stay
        // and the later error lines give way.
        let mut output = errors.clone();
        output.extend([errors[0].clone(), errors[1].clone()]);
        let kept = summary(&output);
        assert!(
            kept.contains("error: e0") && kept.contains("error: e1"),
            "{kept}"
        );
        assert!(
            !kept.contains("error: e3") && !kept.contains("error: e4"),
            "{kept}"
        );
        // A short command leaves room for an error line before the closing
        // lines.
        let mut output = errors.clone();
        output.extend([long("c0"), long("c1")]);
        let Outcome::Failed(failure) = judged(&output.join("\n"), true, None) else {
            panic!("the result failed");
        };
        let kept = summary_of("bash", "make", &failure);
        assert!(kept.len() <= limit, "{}", kept.len());
        assert!(
            kept.contains("error: e0") && kept.ends_with(&closing),
            "{kept}"
        );
        assert!(!kept.contains("error: e4"), "{kept}");
    }

    #[test]
    fn a_structured_result_is_read_by_its_fields() {
        let result = json!({
            "output": "zsh:1: command not found: fd\n",
            "metadata": { "exit_code": 127, "duration_seconds": 0.1, "cached": false },
        });
        let expected = Outcome::Failed(Failure {
            exit_code: Some(127),
            flagged: false,
            lines: vec![Kept {
                text: "zsh:1: command not found: fd".into(),
                closing: true,
            }],
        });
        // The object as the tool's text, and as the value of a JSON result.
        assert_eq!(judged(&result.to_string(), false, None), expected);
        assert_eq!(
            judged_output(&OutputKind::Json { value: result }, None),
            expected
        );
        let passed = json!({ "output": "ok", "metadata": { "exit_code": 0 } }).to_string();
        assert_eq!(judged(&passed, false, None), Outcome::Passed);
        // A field states the exit code before the text does, and the
        // object's own field before a nested one.
        for (result, code) in [
            (
                json!({ "stdout": "child exited with code 3\n", "exit_code": 0 }),
                None,
            ),
            (json!({ "stdout": "", "count": 3, "code": 2 }), Some(2)),
            (
                json!({ "exitCode": 4, "result": { "exit_code": 5 } }),
                Some(4),
            ),
            (json!({ "result": { "status_code": 5 } }), Some(5)),
            (json!({ "exit": "1", "stdout": "ok" }), None),
        ] {
            let outcome = judged_output(
                &OutputKind::Json {
                    value: result.clone(),
                },
                None,
            );
            let read = match outcome {
                Outcome::Failed(failure) => failure.exit_code,
                Outcome::Passed => None,
            };
            assert_eq!(read, code, "{result}");
        }
        // A boolean or a number that states no exit code is no line.
        let quiet = json!({ "error": false, "isError": false, "errors": 0, "stdout": "ok" });
        assert_eq!(judged(&quiet.to_string(), false, None), Outcome::Passed);
        // A JSON string is read as text, line by line.
        let text = OutputKind::Json {
            value: json!("compiling\nerror: bad config"),
        };
        let Outcome::Failed(failure) = judged_output(&text, None) else {
            panic!("the result failed");
        };
        assert_eq!(texts(&failure), ["error: bad config", "compiling"]);
        // Text that only starts like an object is read as text.
        assert_eq!(
            failed("{ not json\nerror: bad config", false),
            Some((
                None,
                vec!["error: bad config".to_owned(), "{ not json".to_owned()]
            ))
        );
    }

    #[test]
    fn every_error_kind_of_output_carries_the_harness_mark() {
        let block = |text: &str| memory_store::ResultBlock {
            kind: ResultBlockKind::Text { text: text.into() },
            provider_extras: Default::default(),
        };
        for (output, flagged) in [
            (
                OutputKind::ErrorJson {
                    value: json!({ "stderr": "denied" }),
                },
                true,
            ),
            (
                OutputKind::Json {
                    value: json!({ "stderr": "denied" }),
                },
                false,
            ),
            (
                OutputKind::ErrorContent {
                    blocks: vec![block("denied")],
                },
                true,
            ),
            (
                OutputKind::Content {
                    blocks: vec![block("denied")],
                },
                false,
            ),
        ] {
            let expected = if flagged {
                Outcome::Failed(Failure {
                    exit_code: None,
                    flagged: true,
                    lines: vec![Kept {
                        text: "denied".into(),
                        closing: true,
                    }],
                })
            } else {
                Outcome::Passed
            };
            assert_eq!(judged_output(&output, None), expected, "{output:?}");
        }
        // A command the harness denied did not run.
        assert!(Reading::of_output(&OutputKind::ExecutionDenied { reason: None }).is_none());
    }

    #[test]
    fn the_record_states_the_outcome_the_command_and_the_lines_within_its_bytes() {
        let line = |text: &str| Kept {
            text: text.into(),
            closing: false,
        };
        let failure = Failure {
            exit_code: Some(101),
            flagged: true,
            lines: vec![line("error[E0308]: mismatched types")],
        };
        assert_eq!(
            summary_of("bash", "cd api && cargo build", &failure),
            "Tool failure recorded from the result of bash: the command failed with exit code 101.\nCommand: cd api && cargo build\nOutput lines:\nerror[E0308]: mismatched types"
        );
        // The longest outcome, tool, command, and closing lines fit with the
        // resolved sentence.
        let full = Failure {
            exit_code: None,
            flagged: false,
            lines: vec![
                Kept {
                    text: "c".repeat(MAX_LINE_BYTES),
                    closing: true
                };
                MAX_CLOSING_LINES
            ],
        };
        let summary = summary_of(&"t".repeat(200), &"m".repeat(400), &full);
        assert!(summary.contains("which states no exit code"));
        assert_eq!(
            summary.matches(&"c".repeat(MAX_LINE_BYTES)).count(),
            MAX_CLOSING_LINES
        );
        assert!(
            summary.len() + RESOLVED_NOTE.len() <= MAX_SUMMARY_BYTES,
            "{}",
            summary.len()
        );
        let command = summary.lines().nth(1).unwrap();
        assert_eq!(command.len(), "Command: ".len() + MAX_COMMAND_BYTES);
        assert!(command.ends_with(CUT_MARK));
    }

    #[test]
    fn runs_order_by_time_and_by_place_within_one_session() {
        let run = |session: &str, at_ms: i64, timed: bool, place: u64| Run {
            at_ms,
            timed,
            session: session.into(),
            place,
        };
        for (later, earlier, follows) in [
            (run("s2", 2_000, true, 1), run("s1", 1_000, true, 9), true),
            (run("s2", 1_000, true, 9), run("s1", 2_000, true, 1), false),
            // Equal times order two sessions' runs in neither direction.
            (run("s2", 1_000, true, 9), run("s1", 1_000, true, 1), false),
            (run("s1", 1_000, true, 9), run("s1", 1_000, true, 1), true),
            (run("s1", 1_000, true, 1), run("s1", 1_000, true, 1), false),
            // A run judged again after a restart carries a later judgment
            // time and its place in the window.
            (
                run("s1", 9_000, false, 3),
                run("s1", 1_000, false, 5),
                false,
            ),
            (run("s1", 9_000, false, 7), run("s1", 1_000, true, 5), true),
        ] {
            assert_eq!(later.follows(&earlier), follows, "{later:?} {earlier:?}");
        }
    }

    #[test]
    fn a_record_id_frames_its_parts_and_the_memo_keeps_its_bounds() {
        assert_ne!(record_id("p", "a:b", "c"), record_id("p", "a", "b:c"));
        assert_eq!(record_id("p", "a", "c"), record_id("p", "a", "c"));
        assert_eq!(record_id("p", "a", "c").len(), 36);
        // A full entry keeps the calls it holds.
        let mut memo = FailureMemo::default();
        let epoch = memo.pin("ses");
        for call in 0..=MEMO_CALLS_PER_SESSION {
            memo.learn("ses", epoch, &format!("call-{call}"));
        }
        assert_eq!(memo.calls_for_test("ses"), MEMO_CALLS_PER_SESSION);
        assert!(memo.has_seen("ses", "call-0"));
        assert!(!memo.has_seen("ses", &format!("call-{MEMO_CALLS_PER_SESSION}")));
        // A new session at the limit takes the place of the oldest session
        // no checkpoint holds.
        for session in 1..MEMO_SESSIONS {
            let session = format!("ses-{session}");
            let epoch = memo.pin(&session);
            memo.release(&session, epoch);
        }
        memo.pin("one-more");
        assert!(memo.has_seen("ses", "call-0"), "a held session stays");
        assert!(!memo.sessions.contains_key("ses-1"));
        assert_eq!(memo.sessions.len(), MEMO_SESSIONS);
        // A deleted session's checkpoint reads its entry as gone, also once
        // the session has a new entry.
        memo.forget_session("ses");
        assert!(!memo.current("ses", epoch));
        let renewed = memo.pin("ses");
        assert!(!memo.current("ses", epoch) && memo.current("ses", renewed));
        memo.learn("ses", epoch, "late");
        assert!(!memo.has_seen("ses", "late"));
    }
}

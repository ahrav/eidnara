//! Project files a user's message named and the agent then used.
//!
//! A message the user sent names a file of the project, and a tool call of
//! the same turn then reads the file or names it in a command it runs. A
//! later session is delivered the file's path, in the `NAMED_FILES` element
//! of its `<project-memory>` block, when the file exists as the block is
//! composed. A record holds the path,
//! the session, and the time of the pass that showed the use, and holds none
//! of the file's content. A delivered line also gives the time the file last
//! changed ([`changes`]), and the `User stated` lines of a read that finds a
//! file to list give the time memory recorded each ([`date_statements`]),
//! so a reader can compare a statement and a file that disagree.
//!
//! Each admitted transform pass of a primary Pi session at the project root
//! shows the turn in progress: the last message of the user and the tool
//! calls after it. A message of the user is a row that Pi gives the user
//! role, the daemon's own synthetic and summary rows left out: a
//! prompt the user types, a template the user invokes, or a prompt an
//! extension sends in the user's name. A text part of it names files when it
//! is text someone typed ([`crate::chunk_text::is_typed_text`]), the test
//! the summarizer's lift evidence applies to a user message.
//! The daemon reads the paths the message names and the calls use from that
//! window alone, keeps no state of the turn, and records each such path once
//! for the session when the path names a regular file of the project.

use std::collections::{HashMap, HashSet, VecDeque};
use std::os::fd::{AsFd as _, BorrowedFd, OwnedFd};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use kernel::{SourceClass, Surface, SurfaceVisibility, TaintClass};
use memory_store::{BlockKind, OutputKind};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::canonical_memory::CanonicalMemory;
use crate::kernel_routes::read::{MAX_READ_ROW_BYTES, MAX_READ_ROWS, RowSelection};
use crate::kernel_routes::{self, KernelOpenCoordinator, ProjectBinding};
use crate::memory_render::NAMED_FILES_SECTION;
use crate::tool_vocabulary::{self, PiOperand};
use crate::{HandlerCore, SessionBinding};

mod changes;

/// How long a pass waits for the history reader's answer about its listed
/// files, for the test that fits the wait inside the plugins' transform
/// deadline.
#[cfg(test)]
pub(crate) const HISTORY_WAIT: std::time::Duration = changes::WAIT;
#[cfg(test)]
pub(crate) use changes::fixtures as change_fixtures;

/// The domain of the use records. The daemon alone writes and changes its
/// rows: `kernel.commit` refuses an operation that names the domain, one of
/// its rows, or a reserved id ([`is_reserved_id`]), and artifact ingest
/// refuses an artifact whose object id is reserved, or that names the
/// domain or the recorder's source kind. The daemon's memory reads select
/// the `memory` domain and the search projection publishes none of these
/// rows, so the capture extractor and `eidnara_search` see none of them.
pub(crate) const DOMAIN_ID: &str = "named-files";
/// The prefixes of a record's object id and decision id.
const ID_PREFIXES: [&str; 2] = ["nfu_", "dnf_"];
/// The prefix of the ids the domain's own row takes when another row holds
/// `domain:named-files`, and how many of them a write tries.
const DOMAIN_ROW_PREFIX: &str = "nfd_";
const DOMAIN_ROW_IDS: usize = 8;
const SOURCE_ID: &str = "named_file_use";
/// The recorder's own source kind. `kernel.commit` derives a caller's
/// classes from a closed list of source kinds that holds no such value, and
/// artifact ingest refuses it, so a row that carries it was written here.
pub(crate) const SOURCE_KIND: &str = "named_file_recorder";

/// Whether `id` is an id the recorder derives: a record's object id or
/// decision id, or an object id of its domain's row. `kernel.commit` and
/// artifact ingest refuse a caller's row under one, which would keep the
/// recorder's own write of that id from committing.
pub(crate) fn is_reserved_id(id: &str) -> bool {
    id.strip_prefix(kernel_routes::commit::DOMAIN_OBJECT_PREFIX) == Some(DOMAIN_ID)
        || id.starts_with(DOMAIN_ROW_PREFIX)
        || ID_PREFIXES.iter().any(|prefix| id.starts_with(prefix))
}
const DECISION_KIND: &str = "named_file_use";

/// Paths one pass hands to a record.
const MAX_PASS_FILES: usize = 8;
/// Distinct path words of one user message that are matched against calls.
const MAX_NAMES_CHECKED: usize = 32;
/// Bytes of text a user message holds at most when it is read for path words.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// A recorded path, in bytes.
const MAX_PATH_BYTES: usize = 256;
/// Path words of one command line that are read.
const MAX_COMMAND_WORDS: usize = 256;
/// Sessions whose recorded paths the memo holds.
const MEMO_SESSIONS: usize = 256;
/// Distinct paths one session records while the memo holds it.
const MAX_SESSION_FILES: usize = 64;
/// Passes whose records are being written at once.
const WRITE_SLOTS: usize = 4;
/// Lines the block's element holds.
pub(crate) const MAX_DELIVERED: usize = 8;
/// Recorded paths whose files one delivery looks for.
const MAX_CHECKED: usize = 32;
/// The time one pass's records wait for the Kernel's writer, together.
const COMMIT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The resident bytes the memo and the write slots hold at their bounds: for
/// each of [`MEMO_SESSIONS`] sessions its ids and [`MAX_SESSION_FILES`] path
/// digests; for each of [`WRITE_SLOTS`] records its project, its session,
/// and [`MAX_PASS_FILES`] paths; and the memo of the files' last changes.
pub(crate) const RETAINED_BYTES_BOUND: u64 = (MEMO_SESSIONS as u64) * 4 * 1024
    + (WRITE_SLOTS as u64) * 16 * 1024
    + changes::RETAINED_BYTES_BOUND;

/// The project's digest and the session's id.
type Owner = (String, String);

/// The paths each session recorded, for the [`MEMO_SESSIONS`] sessions that
/// recorded a path last, and the count of session deletions.
#[derive(Default)]
struct NamedFileMemo {
    /// A digest of each path the session recorded.
    sessions: HashMap<Owner, HashSet<u64>>,
    /// The held sessions, the one that recorded a path longest ago first.
    order: VecDeque<Owner>,
    /// How many session deletions the memo has seen. A pass observed before
    /// a deletion writes no record after it.
    deletes: u64,
}

impl NamedFileMemo {
    fn recorded(&self, owner: &Owner, path: &str) -> bool {
        self.sessions
            .get(owner)
            .is_some_and(|paths| paths.contains(&short_digest(path)))
    }

    /// How many more distinct paths the session may record.
    fn room(&self, owner: &Owner) -> usize {
        MAX_SESSION_FILES.saturating_sub(self.sessions.get(owner).map_or(0, HashSet::len))
    }

    /// Holds that the session recorded `path`, and makes the session the
    /// one that recorded a path last.
    fn record(&mut self, owner: &Owner, path: &str) {
        self.order.retain(|held| held != owner);
        if self.order.len() == MEMO_SESSIONS
            && let Some(oldest) = self.order.pop_front()
        {
            self.sessions.remove(&oldest);
        }
        self.order.push_back(owner.clone());
        self.sessions
            .entry(owner.clone())
            .or_default()
            .insert(short_digest(path));
    }

    fn delete_session(&mut self, session: &str) {
        self.deletes += 1;
        self.sessions.retain(|owner, _| owner.1 != session);
        self.order.retain(|owner| owner.1 != session);
    }
}

/// The sessions' recorded paths and the slots of the records being written.
pub(crate) struct NamedFiles {
    memo: Mutex<NamedFileMemo>,
    slots: Arc<Semaphore>,
}

impl Default for NamedFiles {
    fn default() -> Self {
        Self {
            memo: Mutex::default(),
            slots: Arc::new(Semaphore::new(WRITE_SLOTS)),
        }
    }
}

/// What one submitted window shows: the project paths that the last message
/// of the user names and that a tool call after it used, which the session
/// has not recorded, in the order the calls show them.
#[derive(Debug)]
pub(crate) struct Observed {
    paths: Vec<String>,
    /// The memo's count of session deletions before the window was read.
    deletes: u64,
}

/// SHA-256 over `parts`, each framed by its length.
fn digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn short_digest(text: &str) -> u64 {
    u64::from_le_bytes(
        Sha256::digest(text.as_bytes())[..8]
            .try_into()
            .expect("eight bytes"),
    )
}

fn is_path_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/' | '+' | '@')
}

/// Whether `path` is a project-relative path the recorder writes and the
/// element lists: at most [`MAX_PATH_BYTES`] of ASCII letters, digits, and
/// `._-/+@`, which the confined-path check admits. The check admits no `.`
/// or `..` component, so a recordable path is its own lexical form.
fn is_recordable(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_BYTES
        && path.chars().all(is_path_char)
        && crate::confined_path::check(path).is_ok()
}

/// `raw` as a recordable path of the project, for a session whose working
/// directory is the project root: a relative path without its leading `./`
/// marks, or an absolute path that opens with `root` as the daemon spells
/// it and one `/`, which a root that ends with `/`, as the root `/` does,
/// supplies itself. `raw` holds path characters alone, so a root spelled
/// with another character admits no absolute path, and nothing is
/// normalized: a `.` or `..` component past the leading marks names no file
/// here.
fn project_path<'a>(root: &Path, raw: &'a str) -> Option<&'a str> {
    if !raw.chars().all(is_path_char) {
        return None;
    }
    let rest = if raw.starts_with('/') {
        let root = root.to_str()?;
        let rest = raw.strip_prefix(root)?;
        if root.ends_with('/') {
            rest
        } else {
            rest.strip_prefix('/')?
        }
    } else {
        raw.trim_start_matches("./")
    };
    is_recordable(rest).then_some(rest)
}

/// The runs of path characters in `text`.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|ch| !is_path_char(ch))
}

/// Whether `word` holds a `.` or a `/`, as a path does.
fn is_path_word(word: &&str) -> bool {
    word.contains(['.', '/'])
}

/// Where the sender's own text of a part ends: at the first `<` that an
/// ASCII letter or a `/` follows, where a tag opens. A block another writer
/// adds to a message, such as an attached file or a hint, opens with a tag,
/// on a line of its own or at the end of the sender's line.
fn own_text(part: &str) -> &str {
    let tag = part.match_indices('<').find(|(at, _)| {
        part[at + 1..].starts_with(|next: char| next.is_ascii_alphabetic() || next == '/')
    });
    tag.map_or(part, |(at, _)| &part[..at])
}

/// The project paths a message names. The message's text parts are read
/// when they hold at most [`MAX_MESSAGE_BYTES`] together, each as far as
/// its own text runs ([`own_text`]); a part that is no typed text
/// ([`crate::chunk_text::is_typed_text`]), such as one that opens with a
/// marker of plugin-injected text, names nothing and counts toward the
/// bound. A path word is a run of path characters
/// that holds a `.` or a `/` once the periods that end it are dropped. Each
/// of the first [`MAX_NAMES_CHECKED`] distinct path words names one path: the
/// word as written, or, for a word that opens with Pi's file-reference mark
/// `@`, the word without that one character, as Pi's `read` opens it. So
/// `@docs/a.md` names `docs/a.md`, and `@@scope/pkg.json` and
/// `./@scope/pkg.json` name the file `@scope/pkg.json`.
fn named_paths(root: &Path, message: &crate::wire::IngressMessage) -> Vec<String> {
    let parts: Vec<(&crate::wire::WireBlock, &str)> = message
        .ck
        .content()
        .iter()
        .filter_map(|block| match block.kind() {
            BlockKind::Text { text } => Some((block, text.as_str())),
            _ => None,
        })
        .collect();
    if parts.iter().map(|(_, part)| part.len()).sum::<usize>() > MAX_MESSAGE_BYTES {
        return Vec::new();
    }
    let typed = parts
        .iter()
        .filter(|(block, part)| crate::chunk_text::is_typed_text(block, part));
    let path_words = typed.flat_map(|(_, part)| {
        words(own_text(part))
            .map(|word| word.trim_end_matches('.'))
            .filter(is_path_word)
    });
    let mut seen: HashSet<&str> = HashSet::new();
    let mut named: Vec<String> = Vec::new();
    for word in path_words {
        if seen.len() == MAX_NAMES_CHECKED {
            break;
        }
        if !seen.insert(word) {
            continue;
        }
        if let Some(path) = project_path(root, word.strip_prefix('@').unwrap_or(word))
            && !named.iter().any(|held| held == path)
        {
            named.push(path.to_owned());
        }
    }
    named
}

/// What the observed rows hold for one call id.
#[derive(Default, PartialEq)]
struct Answers {
    calls: usize,
    results: usize,
    /// The id's last result is text, JSON, or content that carries no error.
    succeeded: bool,
    /// A result of the id precedes every call of it, so it answers a call
    /// before the observed rows.
    result_first: bool,
}

impl Answers {
    /// One call, answered after it by one result, which succeeded.
    fn is_one_answered_call(&self) -> bool {
        *self
            == Self {
                calls: 1,
                results: 1,
                succeeded: true,
                result_first: false,
            }
    }
}

/// The ids Pi wrote on the tool calls of an assistant row, in the order of
/// its `toolCall` parts: a part's string `id`, else its string `callId`, as
/// the codec reads them. `None` for a part that holds neither: the codec
/// gives such a call an id of its own making, which pairs it with nothing
/// here.
fn call_ids(row: &Value) -> Vec<Option<&str>> {
    let parts = row.pointer("/message/content").and_then(Value::as_array);
    parts
        .into_iter()
        .flatten()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("toolCall"))
        .map(|part| {
            let id = |key| part.get(key).and_then(Value::as_str);
            id("id").or_else(|| id("callId"))
        })
        .collect()
}

/// The calls and results of `messages`, each with the id Pi wrote on its
/// row: a call's own id, item suffix included, and a result's `toolCallId`.
/// A call or a result whose row holds no such string, and the calls of a
/// row whose `toolCall` parts are not one for each decoded call, are left
/// out. `native` holds the Pi row of each message, in order.
fn identified<'m>(
    messages: &'m [Arc<crate::wire::IngressMessage>],
    native: &'m [Arc<Value>],
) -> Vec<(&'m str, &'m Value, &'m BlockKind)> {
    let mut found = Vec::new();
    for (message, row) in messages.iter().zip(native) {
        let kinds = || message.ck.content().iter().map(|block| block.kind());
        let calls = kinds().filter(|kind| matches!(kind, BlockKind::ToolCall { .. }));
        let ids = call_ids(row);
        if ids.len() == calls.clone().count() {
            for (kind, id) in calls.zip(ids) {
                found.extend(id.map(|id| (id, &**row, kind)));
            }
        }
        let result = row.pointer("/message/toolCallId").and_then(Value::as_str);
        for kind in kinds().filter(|kind| matches!(kind, BlockKind::ToolResult { .. })) {
            found.extend(result.map(|id| (id, &**row, kind)));
        }
    }
    found
}

/// Whether the Pi row of a tool result states that its call succeeded: its
/// `isError` is the boolean `false` and its `content` is an array. The Pi
/// codec reads a result without the flag as a success so that the window
/// still renders; the recorder takes such a result as one whose outcome is
/// unknown, and it shows no use.
fn states_success(row: &Value) -> bool {
    row.pointer("/message/isError") == Some(&Value::Bool(false))
        && row.pointer("/message/content").is_some_and(Value::is_array)
}

/// The files of `named` that the calls of `messages` used. A call of Pi's
/// `read` uses the file its path argument names, which Pi opens without
/// one leading `@`; a call of Pi's `bash` uses the files that the first
/// [`MAX_COMMAND_WORDS`] runs of path characters with a `.` or a `/` in its
/// command line name as written.
/// A call counts when the id Pi wrote on it ([`identified`]) is the id of
/// no other call of `messages` and of one result, which follows the call,
/// is text, JSON, or content that carries no error, and whose Pi row states
/// its success ([`states_success`]). `native` holds the Pi row of each
/// message of `messages`, in order.
fn used_by_calls(
    root: &Path,
    messages: &[Arc<crate::wire::IngressMessage>],
    native: &[Arc<Value>],
    named: &[String],
) -> Vec<String> {
    if native.len() != messages.len() {
        return Vec::new();
    }
    let identified = identified(messages, native);
    let mut ids: HashMap<&str, Answers> = HashMap::new();
    for (id, row, kind) in &identified {
        let answers = ids.entry(id).or_default();
        match kind {
            BlockKind::ToolCall { .. } => answers.calls += 1,
            BlockKind::ToolResult { output, .. } => {
                answers.result_first |= answers.calls == 0;
                answers.results += 1;
                answers.succeeded = states_success(row)
                    && matches!(
                        output.kind,
                        OutputKind::Text { .. }
                            | OutputKind::Json { .. }
                            | OutputKind::Content { .. }
                    );
            }
            _ => {}
        }
    }
    let named_file = |raw: &str| {
        let path = project_path(root, raw)?;
        named.iter().find(|file| *file == path)
    };
    let mut used: Vec<&String> = Vec::new();
    for (id, _, kind) in &identified {
        let BlockKind::ToolCall { name, input, .. } = kind else {
            continue;
        };
        if !ids.get(id).is_some_and(Answers::is_one_answered_call) {
            continue;
        }
        let files: Vec<&String> = match tool_vocabulary::pi_operand(name, input) {
            Some(PiOperand::Read(path)) => named_file(path.strip_prefix('@').unwrap_or(path))
                .into_iter()
                .collect(),
            Some(PiOperand::Command(line)) => words(line)
                .filter(is_path_word)
                .take(MAX_COMMAND_WORDS)
                .filter_map(named_file)
                .collect(),
            None => Vec::new(),
        };
        for file in files {
            if !used.contains(&file) {
                used.push(file);
            }
        }
    }
    used.into_iter().cloned().collect()
}

/// The project's root directory, opened once for a batch of file probes.
fn open_root(root: &Path) -> Option<OwnedFd> {
    use rustix::fs::{Mode, OFlags};
    rustix::fs::open(
        root,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()
}

/// Whether `relative` names a regular file beneath `root`, reached through
/// no symbolic link.
fn is_project_file(root: BorrowedFd<'_>, relative: &str) -> bool {
    use rustix::fs::{FileType, OFlags};
    crate::confined_path::check(relative).is_ok()
        && crate::confined_path::open_beneath(
            root,
            relative,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        )
        .and_then(rustix::fs::fstat)
        .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile)
}

/// Whether the Kernel stores `text` as written: its durable-text redaction
/// finds nothing to replace in it.
fn is_stored_as_written(text: &str) -> bool {
    context_core::redaction::redact_durable_text(text)
        .detections
        .is_empty()
}

/// The files one pass showed in use, to be written off the reply's path.
/// The record holds its write slot until it drops.
pub(crate) struct NamedFileRecord {
    _slot: OwnedSemaphorePermit,
    project: ProjectBinding,
    session: String,
    paths: Vec<String>,
    /// The time of the pass that showed the use.
    used_ms: i64,
    /// The memo's count of session deletions before the pass was observed.
    deletes: u64,
}

impl NamedFileRecord {
    /// Writes a use record for each path, under the gate session deletion
    /// holds, when no session was deleted since the pass was observed. The
    /// gate orders the writers, so each path is written while the session
    /// has a place for it and holds none for it, and is then held as
    /// recorded. A store that does not open writes nothing. Once the gate
    /// is held, one deadline of [`COMMIT_WAIT`] bounds the wait for the
    /// Kernel's writer over all the paths: a path whose commit the Kernel
    /// refuses or that deadline ends stays unrecorded, and the paths after
    /// it are tried under the same deadline. Returns how many it wrote.
    pub(crate) fn write(
        self,
        kernel: &KernelOpenCoordinator,
        gate: &Mutex<()>,
        files: &NamedFiles,
    ) -> usize {
        let Ok(store) = kernel.kernel_store() else {
            eprintln!("daemon: named file records dropped: the Kernel is unavailable");
            return 0;
        };
        let _gate = gate.lock().expect("capture publication mutex");
        if files.memo().deletes != self.deletes {
            return 0;
        }
        let owner = NamedFiles::owner(&self.project, &self.session);
        let deadline = Instant::now() + COMMIT_WAIT;
        let (mut written, mut failed) = (0, 0);
        for path in &self.paths {
            {
                let memo = files.memo();
                if memo.recorded(&owner, path) || memo.room(&owner) == 0 {
                    continue;
                }
            }
            let stored = record(
                &store,
                &self.project,
                &self.session,
                path,
                self.used_ms,
                deadline,
            );
            if stored.is_ok() {
                files.memo().record(&owner, path);
                written += 1;
            } else {
                failed += 1;
            }
        }
        if failed > 0 {
            eprintln!("daemon: named file records: {failed} writes failed");
        }
        written
    }
}

/// The rationale of a record: its source, the session, and the time of the
/// pass that showed the use.
fn rationale(session: &str, used_ms: i64) -> String {
    json!({"source": SOURCE_ID, "session": session, "used_ms": used_ms}).to_string()
}

/// Writes the record that `session` used `path`. The record's id derives
/// from the project, the session, and the path, so a session's second use
/// of a path replays the first commit and leaves the first record as it is.
pub(crate) fn record(
    kernel: &kernel::KernelStore,
    project: &ProjectBinding,
    session: &str,
    path: &str,
    used_ms: i64,
    deadline: Instant,
) -> Result<(), kernel::KernelError> {
    record_as(
        kernel,
        project,
        session,
        path,
        used_ms,
        deadline,
        SOURCE_KIND,
    )
}

/// Registers the `named-files` domain when no live row holds it. The
/// domain's row takes the id `domain:named-files`. In a store where another
/// row holds that id, which a caller could write before the id was
/// reserved, it takes the first of the [`DOMAIN_ROW_IDS`] ids under
/// [`DOMAIN_ROW_PREFIX`] that the registry holds no row for, live or
/// invalidated, and the row that holds an id stays as it is. The records
/// name the domain by `named-files`, whichever id its row has. A store that
/// holds a row under every one of those ids refuses the write with a
/// conflict.
fn ensure_own_domain(envelope: &mut kernel::Envelope<'_>) -> Result<(), kernel::KernelError> {
    if !envelope.domain_exists(DOMAIN_ID)? {
        let preferred = format!("{}{DOMAIN_ID}", kernel_routes::commit::DOMAIN_OBJECT_PREFIX);
        let alternates = (0..DOMAIN_ROW_IDS).map(|index| format!("{DOMAIN_ROW_PREFIX}{index}"));
        let mut free = None;
        for candidate in std::iter::once(preferred).chain(alternates) {
            if envelope.object_state(&candidate)?.is_none() {
                free = Some(candidate);
                break;
            }
        }
        let object_id = free.ok_or(kernel::KernelError::Conflict)?;
        envelope.insert_domain(kernel_routes::commit::domain_spec(DOMAIN_ID, object_id))?;
    }
    Ok(())
}

/// [`record`] with the row's source kind as a parameter, so a test can
/// place a row of another writer's kind in the domain.
fn record_as(
    kernel: &kernel::KernelStore,
    project: &ProjectBinding,
    session: &str,
    path: &str,
    used_ms: i64,
    deadline: Instant,
    source_kind: &str,
) -> Result<(), kernel::KernelError> {
    let identity = digest(&[project.digest(), session, path]);
    let object_id = format!("{}{}", ID_PREFIXES[0], &identity[..32]);
    let intent = kernel::CommitIntent {
        producer: SOURCE_ID.into(),
        operation_key: project
            .operation_key(SOURCE_ID, &object_id)
            .ok_or(kernel::KernelError::InvalidInput)?,
        request_digest: identity.clone(),
        actor: "agent:named_file_use".into(),
        cause: "a project file the user named and the agent used".into(),
    };
    kernel.commit_before(deadline, intent, |envelope| {
        let mut domains = HashSet::new();
        let mut ready = false;
        let mut refusal = None;
        ensure_own_domain(envelope)?;
        kernel_routes::commit::ensure_scope(
            envelope,
            project,
            &mut ready,
            &mut domains,
            &mut refusal,
        )?;
        envelope.insert_decision(kernel::DecisionSpec {
            decision_id: format!("{}{}", ID_PREFIXES[1], &identity[..32]),
            object_id: object_id.clone(),
            domain_id: DOMAIN_ID.into(),
            proposition_id: None,
            scope_id: Some(project.scope_id()),
            anchor_id: None,
            evidence_id: None,
            decision_kind: DECISION_KIND.into(),
            payload: kernel::DecisionPayload {
                summary: path.to_owned(),
                rationale: rationale(session, used_ms),
            },
            source_kind: source_kind.into(),
            source_id: SOURCE_ID.into(),
            source_revision: 1,
            sensitivity: kernel::Sensitivity::Normal,
        })?;
        kernel_routes::commit::admit(
            envelope,
            &object_id,
            (SourceClass::ModelInference, TaintClass::AssistantInference),
        )?;
        Ok("1".to_owned())
    })?;
    Ok(())
}

impl NamedFiles {
    fn memo(&self) -> std::sync::MutexGuard<'_, NamedFileMemo> {
        self.memo.lock().expect("named file memo mutex")
    }

    fn owner(project: &ProjectBinding, session: &str) -> Owner {
        (project.digest().to_owned(), session.to_owned())
    }

    /// Writes the record that `session` used `path` as a row of another
    /// writer's source kind.
    #[cfg(test)]
    pub(crate) fn record_of_another_kind_for_test(
        kernel: &kernel::KernelStore,
        project: &ProjectBinding,
        session: &str,
        path: &str,
    ) {
        let deadline = Instant::now() + COMMIT_WAIT;
        record_as(
            kernel,
            project,
            session,
            path,
            1_000,
            deadline,
            "repository",
        )
        .unwrap();
    }

    /// Takes every place `session` has in `project`.
    #[cfg(test)]
    pub(crate) fn fill_session_for_test(&self, project: &ProjectBinding, session: &str) {
        let owner = Self::owner(project, session);
        for index in 0..MAX_SESSION_FILES {
            self.memo().record(&owner, &format!("filled/{index}.rs"));
        }
    }

    /// Forgets the paths `session` recorded and counts one deletion. The
    /// caller holds the gate the record writes take.
    pub(crate) fn delete_session(&self, session: &str) {
        self.memo().delete_session(session);
    }

    /// The record of what an admitted pass showed: the first
    /// [`MAX_PASS_FILES`] observed paths, within the places the session has
    /// left, that name regular files of the project now and that the Kernel
    /// stores as written. `None` when it holds no path, when the Kernel
    /// would rewrite the record's rationale, when the project root does not
    /// open, when every write slot is taken, and when a session was deleted
    /// since the pass was observed. The caller runs on the blocking pool:
    /// the files are looked for.
    pub(crate) fn keep(
        &self,
        project: &ProjectBinding,
        session: &str,
        observed: &Observed,
        used_ms: i64,
    ) -> Option<NamedFileRecord> {
        if !is_stored_as_written(&rationale(session, used_ms)) {
            return None;
        }
        let owner = Self::owner(project, session);
        // The paths another pass recorded since the observation take none
        // of the session's places.
        let (room, unrecorded) = {
            let memo = self.memo();
            if memo.deletes != observed.deletes {
                return None;
            }
            let unrecorded: Vec<&String> = observed
                .paths
                .iter()
                .filter(|path| !memo.recorded(&owner, path))
                .collect();
            (memo.room(&owner).min(MAX_PASS_FILES), unrecorded)
        };
        let root = open_root(project.root())?;
        let paths: Vec<String> = unrecorded
            .into_iter()
            .filter(|path| is_stored_as_written(path) && is_project_file(root.as_fd(), path))
            .take(room)
            .cloned()
            .collect();
        if paths.is_empty() {
            return None;
        }
        let Ok(slot) = Arc::clone(&self.slots).try_acquire_owned() else {
            eprintln!("daemon: named file records dropped: every write slot is taken");
            return None;
        };
        Some(NamedFileRecord {
            _slot: slot,
            project: project.clone(),
            session: session.to_owned(),
            paths,
            used_ms,
            deletes: observed.deletes,
        })
    }
}

impl HandlerCore {
    /// What the submitted window of a primary Pi session whose
    /// configuration permits memory capture and whose working directory is
    /// the project root shows: the project paths that the last message of
    /// the user names and that a call after it used, except the paths the
    /// session has recorded. The rows before that message can be another
    /// session's, which a fork copies, so they show nothing. Reads the
    /// window and the memo and changes nothing.
    pub(crate) fn observe_named_files(
        &self,
        binding: &SessionBinding,
        request: &crate::transform::TransformRequest,
    ) -> Option<Observed> {
        if request.is_subagent
            || binding.harness != "pi"
            || !crate::memory_capture::capture_enabled(binding)
            || request.working_directory.as_deref() != Some("")
        {
            return None;
        }
        let native = request.native_messages.as_deref().filter(|rows| {
            request.serializer_profile == "pi" && rows.len() == request.messages.len()
        })?;
        let project = &binding.kernel_project;
        // Read before the window, so a deletion that lands while the pass
        // runs is seen when the record is taken and when it is written.
        let deletes = self.named_files.memo().deletes;
        let opener = (0..request.messages.len()).rev().find(|index| {
            let message = &request.messages[*index].ck;
            message.role == "user"
                && native[*index]
                    .pointer("/message/role")
                    .and_then(Value::as_str)
                    == Some("user")
                && !message.meta.synthetic
                && !message.meta.summary
        })?;
        let calls = &request.messages[opener + 1..];
        if calls.is_empty() {
            return None;
        }
        let named = named_paths(project.root(), &request.messages[opener]);
        if named.is_empty() {
            return None;
        }
        let mut paths = used_by_calls(project.root(), calls, &native[opener + 1..], &named);
        let owner = NamedFiles::owner(project, &binding.session);
        {
            let memo = self.named_files.memo();
            paths.retain(|path| !memo.recorded(&owner, path));
        }
        (!paths.is_empty()).then_some(Observed { paths, deletes })
    }

    /// Starts the write of what an admitted pass showed, as a tracked job on
    /// the blocking pool. Runs on the pass's blocking thread once the pass's
    /// transform has succeeded; a daemon that is shutting down drops the
    /// record.
    pub(crate) fn keep_named_files(
        &self,
        binding: &SessionBinding,
        observed: &Observed,
        used_ms: i64,
    ) {
        let Some(record) =
            self.named_files
                .keep(&binding.kernel_project, &binding.session, observed, used_ms)
        else {
            return;
        };
        let kernel = Arc::clone(&self.kernel);
        let gate = Arc::clone(&self.capture_commit_gate);
        let files = Arc::clone(&self.named_files);
        let _ = self
            .admission()
            .spawn_blocking(move || record.write(&kernel, &gate, &files));
    }
}

fn day(at_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at_ms).map_or_else(
        || "an unknown date".to_owned(),
        |at| at.format("%Y-%m-%d").to_string(),
    )
}

/// The lines of the block's `NAMED_FILES` element for a pass of `session`,
/// and whether the read's bounds cut the records short: the files other
/// sessions recorded that are regular files of the project now, the file
/// more sessions used first, then the one used last, then the lesser path,
/// at most [`MAX_DELIVERED`]. Each line holds the path, how many sessions
/// used the file, the day of the last use, and when the file last changed
/// ([`changes::last_changes`]). The read holds the newest
/// records under the read's row and byte bounds, counts the rows this
/// recorder wrote, and looks for the files of the first [`MAX_CHECKED`]
/// paths.
pub(crate) fn delivered(
    store: &kernel::KernelStore,
    project: &ProjectBinding,
    session: &str,
    tip: i64,
) -> Result<(Vec<CanonicalMemory>, bool), kernel::KernelError> {
    #[derive(serde::Deserialize)]
    struct Use {
        session: String,
        used_ms: i64,
    }
    struct File {
        id: String,
        sessions: HashSet<String>,
        last_ms: i64,
    }
    let read = kernel_routes::read::read_visible_within(
        store,
        project,
        Surface::ExplicitSearch,
        Some(tip),
        RowSelection::DomainDecisions(DOMAIN_ID),
        (MAX_READ_ROWS, MAX_READ_ROW_BYTES),
    )?;
    let mut files: HashMap<&str, File> = HashMap::new();
    for row in &read.rows {
        if row.visibility == SurfaceVisibility::Hidden
            || row.object.sensitivity != kernel::Sensitivity::Normal
            || row.object.source_id != SOURCE_ID
            || row.object.source_kind != SOURCE_KIND
        {
            continue;
        }
        let Some(decision) = read.decisions.get(&row.object.object_id) else {
            continue;
        };
        let Ok(used) = serde_json::from_str::<Use>(&decision.payload.rationale) else {
            continue;
        };
        if decision.decision_kind != DECISION_KIND || used.session == session {
            continue;
        }
        let file = files
            .entry(decision.payload.summary.as_str())
            .or_insert_with(|| File {
                id: decision.object_id.clone(),
                sessions: HashSet::new(),
                last_ms: i64::MIN,
            });
        if used.used_ms > file.last_ms {
            file.last_ms = used.used_ms;
            file.id.clone_from(&decision.object_id);
        }
        file.sessions.insert(used.session);
    }
    if files.is_empty() {
        return Ok((Vec::new(), read.truncated));
    }
    let mut ordered: Vec<(&str, File)> = files.into_iter().collect();
    ordered.sort_by(|(left_path, left), (right_path, right)| {
        right
            .sessions
            .len()
            .cmp(&left.sessions.len())
            .then_with(|| right.last_ms.cmp(&left.last_ms))
            .then_with(|| left_path.cmp(right_path))
    });
    let Some(root) = open_root(project.root()) else {
        return Ok((Vec::new(), read.truncated));
    };
    let listed: Vec<(&str, File)> = ordered
        .into_iter()
        .take(MAX_CHECKED)
        .filter(|(path, _)| is_recordable(path) && is_project_file(root.as_fd(), path))
        .take(MAX_DELIVERED)
        .collect();
    let paths: Vec<&str> = listed.iter().map(|(path, _)| *path).collect();
    let changed = changes::last_changes(
        changes::History::shared(),
        project.root(),
        root.as_fd(),
        &paths,
    );
    // A file that left between the two looks at it has no line: each
    // change stays with its own path.
    let lines = listed
        .into_iter()
        .zip(changed)
        .filter_map(|((path, file), change)| {
            Some(CanonicalMemory {
                object_id: file.id,
                category: NAMED_FILES_SECTION.to_owned(),
                content: format!(
                    "{path} (the agent used it in {} earlier session{}, last on {}{LAST_CHANGE_CLAUSE}{})",
                    file.sessions.len(),
                    if file.sessions.len() == 1 { "" } else { "s" },
                    day(file.last_ms),
                    last_change(change?)
                ),
            })
        })
        .collect();
    Ok((lines, read.truncated))
}

/// What opens the clause of a `NAMED_FILES` line that says when the file
/// last changed. A path holds neither `;` nor a space, so the first
/// occurrence in a line opens the clause.
const LAST_CHANGE_CLAUSE: &str = "; last changed ";

/// The part of a `NAMED_FILES` line the block's revision reads: the line up
/// to its last-change clause. The clause is a reading of the work tree when
/// the block was composed, and an edit of a listed file leaves what memory
/// holds, and so the revision, as it was.
pub(crate) fn revised_part(line: &str) -> &str {
    line.split_once(LAST_CHANGE_CLAUSE)
        .map_or(line, |(revised, _)| revised)
}

/// The UTC minute of `at_ms`, as `2026-03-01T09:30Z`, for a time in the
/// years 0000 to 9999, which that form writes with four digits. Any other
/// time is [`UNDATED`].
fn minute(at_ms: i64) -> String {
    use chrono::Datelike as _;
    chrono::DateTime::from_timestamp_millis(at_ms)
        .filter(|at| (0..=9999).contains(&at.year()))
        .map_or_else(
            || UNDATED.to_owned(),
            |at| at.format("%Y-%m-%dT%H:%MZ").to_string(),
        )
}

/// What stands for a time no four-digit year holds.
const UNDATED: &str = "at an unknown time";

/// The words of a line's last-change clause after [`LAST_CHANGE_CLAUSE`]:
/// the minute of the commit that gave the file its bytes; no time for a
/// history the read did not follow to that commit; the minute of the
/// file's modification time and `on disk` for every other file.
fn last_change(change: changes::LastChange) -> String {
    use changes::LastChange;
    match change {
        LastChange::Committed(at_ms) => minute(at_ms),
        LastChange::HistoryUnread => HISTORY_UNREAD.to_owned(),
        LastChange::OnDisk(at_ms) => format!("{} on disk", minute(at_ms)),
    }
}

/// What a line's last-change clause says of a file whose history the read
/// did not follow to the commit that gave the file its bytes.
const HISTORY_UNREAD: &str = "at a time the read history does not show";

/// The clause the packer charges for every `NAMED_FILES` line, whatever
/// the line's own clause holds: the form of the most tokens. The block's
/// selection, and so its revision, then stays as it is when a file's last
/// change moves. `a_clause_costs_at_most_its_charge` checks that no clause
/// [`last_change`] writes costs more, for minutes sampled over the years
/// 0000 to 9999 and for the forms without a minute.
pub(crate) const CHARGED_CLAUSE: &str = "; last changed 2888-08-28T18:58Z on disk)";

/// A statement of the user that a read serves: its id, the commit that
/// first recorded it, and the length of its stored text.
pub(crate) struct UserStatement {
    id: String,
    stated_seq: i64,
    stored_len: usize,
}

/// The user's statements `response` serves: the visible rows capture wrote
/// whose text opens with the label capture gives the user's words and `: `.
/// Taken before a pass rewrites any line, so each holds the length of the
/// text as stored.
pub(crate) fn user_statements(response: &kernel_routes::read::ReadResponse) -> Vec<UserStatement> {
    response
        .rows
        .iter()
        .filter(|row| {
            row.visibility == SurfaceVisibility::Visible
                && crate::memory_capture::is_capture_memory(&row.object)
        })
        .filter_map(|row| {
            let decision = response.decisions.get(&row.object.object_id)?;
            decision
                .payload
                .summary
                .strip_prefix(crate::memory_capture::USER_STATED)
                .is_some_and(|rest| rest.starts_with(": "))
                .then(|| UserStatement {
                    id: decision.object_id.clone(),
                    stated_seq: crate::memory_capture::stated_commit(decision),
                    stored_len: decision.payload.summary.len(),
                })
        })
        .collect()
}

/// Appends ` [<minute>]` to the line of each of `statements` in
/// `response`: the UTC minute of the commit that first recorded the
/// statement, read from the store in one statement for the pass. Capture
/// records a statement after the user made it, so the statement was made at
/// that minute or before it. A failed read fails the pass. The rewritten
/// lines are the pass's facts wherever the pass's read serves them: the
/// block, and the summarizer prompt of a firing that read serves, as a
/// contest mark is.
pub(crate) fn date_statements(
    store: &kernel::KernelStore,
    response: &mut kernel_routes::read::ReadResponse,
    statements: &[UserStatement],
) -> Result<(), kernel::KernelError> {
    if statements.is_empty() {
        return Ok(());
    }
    let mut sequences: Vec<i64> = statements
        .iter()
        .map(|statement| statement.stated_seq)
        .collect();
    sequences.sort_unstable();
    sequences.dedup();
    let times = store.commits_recorded_at(&sequences)?;
    mark_recorded(response, statements, &times);
    Ok(())
}

/// Appends to the line of each of `statements` the minute `recorded_ms`
/// holds for the commit that first recorded it. A line another mark
/// already rewrote, which is longer than the stored text, is left as that
/// mark wrote it: a contest mark dates its statement itself. A statement
/// whose commit `recorded_ms` holds no time for keeps its stored text.
pub(crate) fn mark_recorded(
    response: &mut kernel_routes::read::ReadResponse,
    statements: &[UserStatement],
    recorded_ms: &HashMap<i64, i64>,
) {
    for statement in statements {
        let (Some(decision), Some(at_ms)) = (
            response.decisions.get_mut(&statement.id),
            recorded_ms.get(&statement.stated_seq),
        ) else {
            continue;
        };
        if decision.payload.summary.len() == statement.stored_len {
            decision
                .payload
                .summary
                .push_str(&format!(" [{}]", minute(*at_ms)));
        }
    }
}
#[cfg(test)]
mod bounds_tests {
    use super::*;

    fn project(files: &[&str]) -> (tempfile::TempDir, ProjectBinding) {
        let dir = tempfile::tempdir().unwrap();
        for file in files {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "text\n").unwrap();
        }
        let project = ProjectBinding::new(dir.path());
        (dir, project)
    }

    fn owner(session: &str) -> Owner {
        ("project".to_owned(), session.to_owned())
    }

    fn message(role: &str, blocks: Vec<BlockKind>) -> crate::wire::IngressMessage {
        use crate::wire::{HarnessMeta, ProviderExtras, WireBlock, WireMessage};
        crate::wire::IngressMessage {
            mid: "m".to_owned(),
            ordinal: 1,
            ck: WireMessage::from_parts(
                role,
                blocks.into_iter().map(WireBlock::bare).collect(),
                None,
                ProviderExtras::new(),
                HarnessMeta::default(),
            ),
        }
    }

    fn user(text: &str) -> crate::wire::IngressMessage {
        message("user", vec![BlockKind::Text { text: text.into() }])
    }

    /// A row of a window: a message and the Pi row it was decoded from.
    type Row = (Arc<crate::wire::IngressMessage>, Arc<Value>);

    fn pi_row(message: Value) -> Arc<Value> {
        Arc::new(json!({ "message": message }))
    }

    /// A call of `name` under `id`, which its Pi row holds as the id of its
    /// one `toolCall` part.
    fn call_row(id: &str, name: &str, input: Value) -> Row {
        let part = json!({ "type": "toolCall", "id": id, "name": name, "arguments": input });
        let block = BlockKind::ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            input,
            provider_executed: false,
        };
        (
            Arc::new(message("assistant", vec![block])),
            pi_row(json!({ "role": "assistant", "content": [part] })),
        )
    }

    /// A result under `id` whose Pi row is `row`, which names the id as
    /// its `toolCallId` unless it names one itself.
    fn result_row(id: &str, output: OutputKind, mut row: Value) -> Row {
        let block = BlockKind::ToolResult {
            id: id.to_owned(),
            tool_name: "tool".to_owned(),
            output: memory_store::ToolOutput::bare(output),
            provider_executed: false,
        };
        if row.get("toolCallId").is_none() {
            row["toolCallId"] = json!(id);
        }
        (Arc::new(message("tool", vec![block])), pi_row(row))
    }

    /// The Pi row of a result that states `is_error`.
    fn stated(is_error: bool) -> Value {
        json!({ "role": "toolResult", "content": [], "isError": is_error })
    }

    /// A call of `name` under `id` and the result the tool answered it
    /// with, whose Pi row states an error for an error kind of `output`.
    fn answered_as(id: &str, name: &str, input: Value, output: OutputKind) -> Vec<Row> {
        let succeeded = matches!(
            output,
            OutputKind::Text { .. } | OutputKind::Json { .. } | OutputKind::Content { .. }
        );
        vec![
            call_row(id, name, input),
            result_row(id, output, stated(!succeeded)),
        ]
    }

    fn answered(name: &str, input: Value, output: OutputKind) -> Vec<Row> {
        answered_as("call-1", name, input, output)
    }

    fn call(name: &str, input: Value) -> Vec<Row> {
        answered(name, input, OutputKind::Text { text: "ok".into() })
    }

    /// The files of `named` that the calls of `rows` used.
    fn used(rows: &[Row], named: &[String]) -> Vec<String> {
        let (messages, native): (Vec<_>, Vec<_>) = rows.iter().cloned().unzip();
        used_by_calls(root(), &messages, &native, named)
    }

    const ROOT: &str = "/work/project";

    fn root() -> &'static Path {
        Path::new(ROOT)
    }

    /// A path is held relative to the project root: a relative path without
    /// its leading `./` marks, or an absolute path that opens with the root
    /// as the daemon spells it. Nothing is normalized: a path with a `.` or
    /// `..` component past those marks, an absolute path with a repeated
    /// separator, a path of the Git store, a path past 256 bytes, and a
    /// word with another character name no file. A root spelled with
    /// another character admits relative paths alone.
    #[test]
    fn a_path_is_a_plain_path_beneath_the_root_within_256_bytes() {
        fn path(raw: &str) -> Option<&str> {
            project_path(root(), raw)
        }
        assert_eq!(path("docs/a.md"), Some("docs/a.md"));
        assert_eq!(path("./docs/a.md"), Some("docs/a.md"));
        assert_eq!(path("././tools/check.sh"), Some("tools/check.sh"));
        assert_eq!(path("/work/project/docs/a.md"), Some("docs/a.md"));
        for refused in [
            "docs/./a.md",
            "docs/../README.md",
            "../project/docs/a.md",
            "/work/other/docs/a.md",
            "/work/projects/docs/a.md",
            "/work/project/../project/docs/a.md",
            "/work/project/./docs/a.md",
            "/work/project//docs/a.md",
            "//work/project/docs/a.md",
            "/work/project",
            "/work/project/",
            ".git/config",
            "docs/a b.md",
            "\"docs/a.md\"",
            "docs//a.md",
            "docs/",
            ".",
            "./",
            "",
        ] {
            assert_eq!(path(refused), None, "{refused:?}");
        }
        let longest = format!("{}/{}.md", "a".repeat(100), "b".repeat(152));
        assert_eq!(longest.len(), 256);
        assert_eq!(path(&longest), Some(longest.as_str()));
        assert_eq!(path(&format!("./{longest}")), Some(longest.as_str()));
        assert_eq!(path(&format!("{ROOT}/{longest}")), Some(longest.as_str()));
        assert_eq!(path(&format!("{longest}x")), None);

        // Pi's `read` rewrites a no-break space of an absolute path to a
        // space before it opens the file, so the operand names another root.
        for spaced in ["/work/my project", "/work/my\u{a0}project"] {
            let spaced_root = Path::new(spaced);
            assert_eq!(
                project_path(spaced_root, &format!("{spaced}/docs/a.md")),
                None
            );
            assert_eq!(project_path(spaced_root, "docs/a.md"), Some("docs/a.md"));
        }

        // A root that ends with `/`, as the root `/` does, supplies the
        // separator itself, and the rest of the path is read as written.
        let top = Path::new("/");
        assert_eq!(project_path(top, "/docs/a.md"), Some("docs/a.md"));
        assert_eq!(project_path(top, "docs/a.md"), Some("docs/a.md"));
        assert_eq!(project_path(top, "//docs/a.md"), None);
        assert_eq!(project_path(top, "/"), None);
        assert_eq!(project_path(top, "/docs/../a.md"), None);
        let slashed = Path::new("/work/project/");
        assert_eq!(
            project_path(slashed, "/work/project/docs/a.md"),
            Some("docs/a.md")
        );
        assert_eq!(project_path(slashed, "/work/project//docs/a.md"), None);
        assert_eq!(project_path(slashed, "/work/projectdocs/a.md"), None);
        // The same rule reads a message's word and a call's operand.
        assert_eq!(
            named_paths(top, &user("Follow /docs/a.md and //docs/b.md.")),
            ["docs/a.md"]
        );
        let (messages, native): (Vec<_>, Vec<_>) = call("read", json!({"path": "/docs/a.md"}))
            .into_iter()
            .unzip();
        assert_eq!(
            used_by_calls(top, &messages, &native, &["docs/a.md".to_owned()]),
            ["docs/a.md"]
        );
    }

    /// A message's path words are its runs of path characters that hold a
    /// `.` or a `/` once the periods that end them are dropped. The first
    /// 32 distinct words are read, whatever they name; a repeated word
    /// takes no second place.
    #[test]
    fn the_first_32_distinct_path_words_of_a_message_are_read() {
        assert_eq!(
            named_paths(
                root(),
                &user("Read docs/a.md. Then run `./tools/check.sh`, twice... v1.2 is out.")
            ),
            ["docs/a.md", "tools/check.sh", "v1.2"]
        );
        let refused: Vec<String> = (0..30).map(|index| format!("../out-{index}.md")).collect();
        // Thirty refused words, one of them twice, then docs/a.md as the
        // 31st distinct word, docs/b.md as the 32nd, and docs/c.md past the
        // bound.
        let text = format!(
            "{} ../out-0.md docs/a.md docs/b.md docs/c.md",
            refused.join(" ")
        );
        assert_eq!(
            named_paths(root(), &user(&text)),
            ["docs/a.md", "docs/b.md"]
        );
    }

    /// Each text part of a message is read as far as the sender's own text
    /// runs: to the first `<` that a letter or a `/` follows, where a block
    /// another writer added opens, on its own line or at the end of the
    /// sender's line. A `<` that anything else follows is the sender's.
    #[test]
    fn a_text_part_is_read_up_to_its_first_tag() {
        let names = |text: &str| named_paths(root(), &user(text));
        assert_eq!(
            names(
                "Follow docs/a.md.\n```\ntools/check.sh < in.txt && a <= b\n```\n\
                 <context-hint>\nSee docs/hinted.md.\n</context-hint>\nThen docs/after.md."
            ),
            ["docs/a.md", "tools/check.sh", "in.txt"]
        );
        // Pi joins an attached file's block to piped input with no line
        // break, and the attached path can hold a `>`.
        assert_eq!(
            names("Apply docs/a.md<file name=\"/work/project/docs/g>h.md\">\ndocs/b.md\n</file>"),
            ["docs/a.md"]
        );
        assert_eq!(
            names("<skill name=\"x\" location=\"a>b\">\ndocs/b.md\n</skill>\ndocs/c.md"),
            [""; 0]
        );
        assert_eq!(names("docs/a.md\n</stray>\ndocs/b.md"), ["docs/a.md"]);
        // An open code fence holds a later block no longer than its tag.
        assert_eq!(
            names("docs/a.md\n```\ndocs/c.md\n<aug>\ndocs/d.md"),
            ["docs/a.md", "docs/c.md"]
        );
        assert_eq!(names("Fix <b>docs/b.md</b> and docs/c.md."), [""; 0]);
        // A tag opens with a letter of either case. A `<` that a digit, a
        // space, `=`, `-`, or the text's end follows is the sender's.
        assert_eq!(names("docs/a.md <File>\ndocs/b.md"), ["docs/a.md"]);
        assert_eq!(names("docs/a.md </B> docs/b.md"), ["docs/a.md"]);
        assert_eq!(
            names("a.txt <3 b.txt <- c.txt <= d.txt < e.txt <"),
            ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]
        );
        // A letter outside ASCII opens no tag.
        assert_eq!(names("a.txt <\u{e9}t\u{e9}> b.txt"), ["a.txt", "b.txt"]);
        // Each part is read on its own: a block that ends the first part
        // leaves the second part's text the sender's.
        let parts = message(
            "user",
            vec![
                BlockKind::Text {
                    text: "Follow docs/a.md.\n<context-hint>\ndocs/hinted.md".into(),
                },
                BlockKind::Text {
                    text: "Then docs/b.md.".into(),
                },
            ],
        );
        assert_eq!(named_paths(root(), &parts), ["docs/a.md", "docs/b.md"]);
    }

    /// A message whose text parts hold more than 65,536 bytes together
    /// names no file; one of exactly that many bytes is read whole. The
    /// bound counts bytes, and the parts alone.
    #[test]
    fn a_message_past_64_kib_of_text_names_no_file() {
        let filler = "x ".repeat(32_763);
        let whole = format!("{filler}docs/a.md ");
        assert_eq!(whole.len(), 65_536);
        assert_eq!(named_paths(root(), &user(&whole)), ["docs/a.md"]);
        assert!(named_paths(root(), &user(&format!("{whole} "))).is_empty());
        // Two-byte characters: 65,536 bytes in 32,773 characters, and one
        // byte more with a further letter.
        let accented = format!("docs/a.md {}", "\u{e9}".repeat(32_763));
        assert_eq!((accented.len(), accented.chars().count()), (65_536, 32_773));
        assert_eq!(named_paths(root(), &user(&accented)), ["docs/a.md"]);
        assert!(named_paths(root(), &user(&format!("{accented}x"))).is_empty());
        // Parts of 65,535 bytes and 1 byte are read; a third part of one
        // more byte passes the bound.
        let parts = |last: &str| {
            message(
                "user",
                vec![
                    BlockKind::Text {
                        text: whole[..65_535].to_owned(),
                    },
                    BlockKind::Text {
                        text: String::new(),
                    },
                    BlockKind::Text { text: last.into() },
                ],
            )
        };
        assert_eq!(named_paths(root(), &parts("x")), ["docs/a.md"]);
        assert!(named_paths(root(), &parts("xy")).is_empty());
        // The bytes after a part's first tag count too: the same path
        // before a block of 65,536 bytes in all, and of one byte more.
        let before = "docs/a.md\n<file>\n";
        let tagged = |bytes: usize| format!("{before}{}", "y".repeat(bytes - before.len()));
        assert_eq!(named_paths(root(), &user(&tagged(65_536))), ["docs/a.md"]);
        assert!(named_paths(root(), &user(&tagged(65_537))).is_empty());
    }

    /// A word that opens with Pi's `@` names the path without that one
    /// character, as Pi's `read` opens its path argument, and a command
    /// line names a path as written, periods included. A second `@`, and a
    /// leading `./`, keep an `@` that belongs to the path.
    #[test]
    fn a_pi_file_reference_names_the_file_pi_opens() {
        let named = named_paths(
            root(),
            &user("See @docs/a.md, @@scope/pkg.json and ./@types/x.d.ts."),
        );
        assert_eq!(named, ["docs/a.md", "@scope/pkg.json", "@types/x.d.ts"]);
        let used = |rows: Vec<Row>| used(&rows, &named);
        assert_eq!(
            used(call("read", json!({"path": "@docs/a.md"}))),
            ["docs/a.md"]
        );
        assert_eq!(
            used(call("read", json!({"path": "@@scope/pkg.json"}))),
            ["@scope/pkg.json"]
        );
        assert_eq!(
            used(call("bash", json!({"command": "cat @scope/pkg.json"}))),
            ["@scope/pkg.json"]
        );
        // A single mark names the unmarked path alone: the marked path is
        // another file, which this message did not name.
        assert!(used(call("read", json!({"path": "@@docs/a.md"}))).is_empty());
        assert!(used(call("bash", json!({"command": "cat @docs/a.md"}))).is_empty());
        assert!(used(call("bash", json!({"command": "cat docs/a.md."}))).is_empty());
        assert!(used(call("bash", json!({"command": "cat x/../docs/a.md"}))).is_empty());
    }

    /// A call and a result pair by the id Pi wrote. The codec shortens an
    /// id with an item suffix to the part before its `|`: two calls that
    /// share that part and differ in the suffix are two calls, each paired
    /// with the result of its own id, and a result whose suffix differs
    /// from its call's answers no call.
    #[test]
    fn a_call_and_a_result_pair_by_the_id_pi_wrote() {
        let named = ["docs/a.md".to_owned(), "docs/b.md".to_owned()];
        let read = |row: &str, id: &str, path: &str| {
            json!({ "id": row, "message": {
                "role": "assistant",
                "content": [{
                    "type": "toolCall", "id": id, "name": "read", "arguments": { "path": path }
                }],
                "usage": {}, "stopReason": "toolUse", "timestamp": 1
            } })
        };
        let result = |row: &str, id: &str, is_error: bool| {
            json!({ "id": row, "message": {
                "role": "toolResult", "toolCallId": id, "toolName": "read",
                "content": [{ "type": "text", "text": "text" }], "isError": is_error,
                "timestamp": 2
            } })
        };
        let used = |rows: Vec<Value>| {
            let rows: Vec<Arc<Value>> = rows.into_iter().map(Arc::new).collect();
            let messages: Vec<_> = crate::codec::pi::decode_pi_rows(&rows)
                .unwrap()
                .messages
                .into_iter()
                .map(Arc::new)
                .collect();
            used_by_calls(root(), &messages, &rows, &named)
        };
        assert_eq!(
            used(vec![
                read("00000001", "call-1|item-a", "docs/a.md"),
                result("00000002", "call-1|item-a", false),
                read("00000003", "call-1|item-b", "docs/b.md"),
                result("00000004", "call-1|item-b", false),
            ]),
            ["docs/a.md", "docs/b.md"]
        );
        assert_eq!(
            used(vec![
                read("00000001", "call-1|item-a", "docs/a.md"),
                result("00000002", "call-1|item-a", true),
                read("00000003", "call-1|item-b", "docs/b.md"),
                result("00000004", "call-1|item-b", false),
            ]),
            ["docs/b.md"]
        );
        assert!(
            used(vec![
                read("00000001", "call-1|item-a", "docs/a.md"),
                result("00000002", "call-1|item-b", false),
            ])
            .is_empty()
        );
        assert_eq!(
            used(vec![
                read("00000001", "call-1", "docs/a.md"),
                result("00000002", "call-1", false),
            ]),
            ["docs/a.md"]
        );

        // A row that holds no id gets one of the codec's making, and that
        // id pairs nothing: a result without a `toolCallId` decodes under
        // the id `tool`, as a call Pi named `tool` does, and a call without
        // an id decodes under an id the codec derives.
        let without = |mut row: Value, key: &str| {
            let message = row["message"].as_object_mut().unwrap();
            match message
                .get_mut("content")
                .and_then(|parts| parts.get_mut(0))
            {
                Some(part) if key == "id" => {
                    part.as_object_mut().unwrap().remove("id");
                }
                _ => {
                    message.remove(key);
                }
            }
            row
        };
        assert_eq!(
            used(vec![
                read("00000001", "tool", "docs/a.md"),
                result("00000002", "tool", false),
            ]),
            ["docs/a.md"]
        );
        assert!(
            used(vec![
                read("00000001", "tool", "docs/a.md"),
                without(result("00000002", "tool", false), "toolCallId"),
            ])
            .is_empty()
        );
        let mut numbered = result("00000002", "tool", false);
        numbered["message"]["toolCallId"] = json!(7);
        assert!(used(vec![read("00000001", "tool", "docs/a.md"), numbered]).is_empty());
        let unnamed = without(read("00000001", "call-1", "docs/a.md"), "id");
        let derived = {
            let rows = vec![Arc::new(unnamed.clone())];
            let decoded = crate::codec::pi::decode_pi_rows(&rows).unwrap().messages;
            let Some(BlockKind::ToolCall { id, .. }) =
                decoded[0].ck.content().first().map(|block| block.kind())
            else {
                panic!("the row decodes to a call");
            };
            id.clone()
        };
        assert!(used(vec![unnamed, result("00000002", &derived, false)]).is_empty());
        // A `callId` names a call that holds no `id`.
        let mut called = without(read("00000001", "call-1", "docs/a.md"), "id");
        called["message"]["content"][0]["callId"] = json!("call-9");
        assert_eq!(
            used(vec![called, result("00000002", "call-9", false)]),
            ["docs/a.md"]
        );
    }

    /// Pi's `read` uses the file its path names, and Pi's `bash` the files
    /// the first 256 path words of its command line name. A call of another
    /// tool, a call whose result is an error of any kind, and a call with
    /// no result in the window use nothing.
    #[test]
    fn a_read_or_a_command_that_was_answered_uses_a_file() {
        let named = ["docs/a.md".to_owned(), "docs/b.md".to_owned()];
        let command = |filler: usize| {
            let words: Vec<String> = (0..filler).map(|index| format!("f{index}.txt")).collect();
            call(
                "bash",
                json!({"command": format!("cat {} docs/a.md", words.join(" "))}),
            )
        };
        assert_eq!(used(&command(255), &named), ["docs/a.md"]);
        assert!(used(&command(256), &named).is_empty());
        let both: Vec<Row> = [
            answered_as(
                "call-1",
                "read",
                json!({"path": "docs/b.md"}),
                OutputKind::Json { value: json!({}) },
            ),
            answered_as(
                "call-2",
                "read",
                json!({"path": "./docs/a.md", "filePath": "docs/c.md"}),
                OutputKind::Content { blocks: Vec::new() },
            ),
        ]
        .concat();
        assert_eq!(used(&both, &named), ["docs/b.md", "docs/a.md"]);
        let failed = |output: OutputKind| answered("read", json!({"path": "docs/a.md"}), output);
        for unused in [
            call(
                "edit",
                json!({"path": "docs/a.md", "oldText": "a", "newText": "b"}),
            ),
            call("ls", json!({"path": "docs/a.md"})),
            call("Read", json!({"path": "docs/a.md"})),
            call("read", json!({"filePath": "docs/a.md"})),
            failed(OutputKind::ErrorText {
                text: "no such file".into(),
            }),
            failed(OutputKind::ErrorJson { value: json!({}) }),
            failed(OutputKind::ErrorContent { blocks: Vec::new() }),
            failed(OutputKind::ExecutionDenied { reason: None }),
            vec![call_row("call-9", "read", json!({"path": "docs/a.md"}))],
        ] {
            assert!(used(&unused, &named).is_empty(), "{unused:?}");
        }
        // A window whose Pi rows are not one for each message shows no use.
        let (messages, native): (Vec<_>, Vec<_>) = call("read", json!({"path": "docs/a.md"}))
            .into_iter()
            .unzip();
        assert!(used_by_calls(root(), &messages, &native[..1], &named).is_empty());
    }

    /// A result shows its call's success when its Pi row states it: the
    /// row's `isError` is the boolean `false` and its `content` is an
    /// array. A row without the flag, with a flag of another type, or
    /// without a content array leaves the outcome unknown, and the call
    /// uses nothing, for `read` and for `bash`. Such a result still counts
    /// as a result of its id, and leaves another id's call as it is.
    #[test]
    fn a_result_shows_success_when_its_pi_row_states_it() {
        let named = ["docs/a.md".to_owned(), "docs/b.md".to_owned()];
        let ok = || OutputKind::Text { text: "ok".into() };
        let with = |name: &str, input: Value, row: Value| {
            vec![
                call_row("call-1", name, input),
                result_row("call-1", ok(), row),
            ]
        };
        let read = |row: Value| with("read", json!({"path": "docs/a.md"}), row);
        let ran = |row: Value| with("bash", json!({"command": "cat docs/a.md"}), row);
        let text = json!([{"type": "text", "text": "ok"}]);
        for stating in [
            json!({"role": "toolResult", "content": [], "isError": false}),
            json!({"role": "toolResult", "content": text, "isError": false}),
            json!({"role": "toolResult", "content": [{"type": "image"}], "isError": false}),
        ] {
            assert_eq!(
                used(&read(stating.clone()), &named),
                ["docs/a.md"],
                "{stating}"
            );
            assert_eq!(
                used(&ran(stating.clone()), &named),
                ["docs/a.md"],
                "{stating}"
            );
        }
        for unknown in [
            json!({"role": "toolResult", "content": text}),
            json!({"role": "toolResult", "content": text, "isError": null}),
            json!({"role": "toolResult", "content": text, "isError": "false"}),
            json!({"role": "toolResult", "content": text, "isError": 0}),
            json!({"role": "toolResult", "content": text, "isError": true}),
            json!({"role": "toolResult", "isError": false}),
            json!({"role": "toolResult", "content": "ok", "isError": false}),
            json!({"role": "toolResult", "content": {"0": "ok"}, "isError": false}),
            json!({}),
        ] {
            assert!(used(&read(unknown.clone()), &named).is_empty(), "{unknown}");
            assert!(used(&ran(unknown.clone()), &named).is_empty(), "{unknown}");
        }
        // A result of unknown outcome beside a stated success of the same
        // id leaves the id with two results; another id's pair counts.
        let mut rows = read(json!({"role": "toolResult", "content": []}));
        rows.push(result_row("call-1", ok(), stated(false)));
        rows.extend(answered_as(
            "call-2",
            "read",
            json!({"path": "docs/b.md"}),
            ok(),
        ));
        assert_eq!(used(&rows, &named), ["docs/b.md"]);
    }

    /// A call counts when its id is one call's and one result's, and the
    /// result follows the call. Two calls under one id, a call with two
    /// results, and a call whose id an earlier result carries use nothing:
    /// the rows do not say which result answers which call.
    #[test]
    fn a_call_counts_when_its_id_pairs_it_with_one_later_result() {
        let named = ["docs/a.md".to_owned(), "docs/b.md".to_owned()];
        let read = |id: &str, path: &str| call_row(id, "read", json!({ "path": path }));
        let ok = |id: &str| result_row(id, OutputKind::Text { text: "ok".into() }, stated(false));
        let failed = |id: &str| {
            result_row(
                id,
                OutputKind::ErrorText {
                    text: "no such file".into(),
                },
                stated(true),
            )
        };
        let used = |rows: &[Row]| used(rows, &named);
        let (a, b) = (read("call-1", "docs/a.md"), read("call-1", "docs/b.md"));
        assert_eq!(used(&[a.clone(), ok("call-1")]), ["docs/a.md"]);
        // Two calls under one id, in either order of their results.
        assert!(used(&[a.clone(), failed("call-1"), b.clone(), ok("call-1")]).is_empty());
        assert!(used(&[b.clone(), ok("call-1"), a.clone(), failed("call-1")]).is_empty());
        assert!(used(&[a.clone(), b.clone(), ok("call-1")]).is_empty());
        // One call with two results.
        assert!(used(&[a.clone(), ok("call-1"), ok("call-1")]).is_empty());
        assert!(used(&[a.clone(), failed("call-1"), ok("call-1")]).is_empty());
        // A result before the call answers a call before the observed rows.
        assert!(used(&[ok("call-1"), a.clone()]).is_empty());
        // Another id's rows leave the pair as it is.
        assert_eq!(
            used(&[
                a,
                failed("call-1"),
                read("call-2", "docs/b.md"),
                ok("call-2")
            ]),
            ["docs/b.md"]
        );
    }

    /// A text part names files when it is text someone typed. A part that
    /// opens with a marker of plugin-injected text, and the text the Pi
    /// codec gives a bash execution, name nothing, and the typed parts
    /// beside them are read.
    #[test]
    fn a_part_that_is_no_typed_text_names_no_file() {
        let parts = |texts: &[&str]| {
            message(
                "user",
                texts
                    .iter()
                    .map(|text| BlockKind::Text {
                        text: (*text).to_owned(),
                    })
                    .collect(),
            )
        };
        assert_eq!(
            named_paths(root(), &parts(&["Follow docs/a.md."])),
            ["docs/a.md"]
        );
        for injected in [
            "[SYSTEM DIRECTIVE: REVIEW] Open docs/a.md and report.",
            "Ran `cat docs/a.md`\n```\nthe text of the file\n```",
            "Ran `ls docs/a.md`\n(no output)",
        ] {
            assert!(
                named_paths(root(), &parts(&[injected])).is_empty(),
                "{injected:?}"
            );
            assert_eq!(
                named_paths(root(), &parts(&[injected, "Then read docs/b.md."])),
                ["docs/b.md"],
                "{injected:?}"
            );
        }
        // A part that names nothing counts toward the 65,536 bytes: with a
        // typed part of 15 bytes, an injected part of 65,521 bytes fills
        // the bound and one byte more passes it.
        let typed = "Read docs/b.md.";
        let marker = "[SYSTEM DIRECTIVE: REVIEW] ";
        let injected = |bytes: usize| format!("{marker}{}", "x".repeat(bytes - marker.len()));
        assert_eq!(typed.len(), 15);
        assert_eq!(
            named_paths(root(), &parts(&[&injected(65_521), typed])),
            ["docs/b.md"]
        );
        assert!(named_paths(root(), &parts(&[&injected(65_522), typed])).is_empty());
    }

    /// The memo holds the 256 sessions that recorded a path last, and a
    /// session has room for 64 distinct paths.
    #[test]
    fn the_memo_holds_256_sessions_and_64_paths_of_each() {
        let mut memo = NamedFileMemo::default();
        for index in 0..256 {
            memo.record(&owner(&format!("s{index}")), "docs/a.md");
        }
        memo.record(&owner("s0"), "docs/b.md");
        memo.record(&owner("s256"), "docs/a.md");
        assert_eq!((memo.sessions.len(), memo.order.len()), (256, 256));
        assert!(memo.recorded(&owner("s0"), "docs/a.md"));
        assert!(!memo.recorded(&owner("s1"), "docs/a.md"));
        assert!(memo.recorded(&owner("s256"), "docs/a.md"));
        assert_eq!(memo.room(&owner("s0")), 62);
        assert_eq!(memo.room(&owner("s1")), 64);
        // A session past its places has none left.
        for index in 0..70 {
            memo.record(&owner("s0"), &format!("src/{index}.rs"));
        }
        assert_eq!(memo.room(&owner("s0")), 0);
    }

    /// A session's paths are held for each project, and a deletion forgets
    /// the session's paths in every project and no other session's.
    #[test]
    fn recorded_paths_belong_to_a_project_and_a_deletion_forgets_the_session() {
        let mut memo = NamedFileMemo::default();
        let here = ("here".to_owned(), "s".to_owned());
        let there = ("there".to_owned(), "s".to_owned());
        memo.record(&here, "docs/a.md");
        memo.record(&owner("other"), "docs/a.md");
        assert!(memo.recorded(&here, "docs/a.md"));
        assert!(!memo.recorded(&there, "docs/a.md"));
        memo.record(&there, "docs/a.md");
        memo.delete_session("s");
        assert_eq!(memo.deletes, 1);
        assert!(!memo.recorded(&here, "docs/a.md"));
        assert!(!memo.recorded(&there, "docs/a.md"));
        assert!(memo.recorded(&owner("other"), "docs/a.md"));
    }

    /// Four passes' records are written at once. A pass that finds every
    /// slot taken is dropped, and a slot is free again once its record
    /// drops. A record holds the first eight observed paths that are
    /// regular files and that the Kernel stores as written, within the
    /// places the session has left.
    #[test]
    fn a_pass_takes_one_of_four_write_slots_and_eight_paths() {
        let names: Vec<String> = (0..10).map(|index| format!("docs/g{index}.md")).collect();
        let on_disk: Vec<&str> = names.iter().map(String::as_str).collect();
        let secret = "keys/sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678.txt".to_owned();
        let mut on_disk: Vec<&str> = on_disk;
        on_disk.push(&secret);
        let (dir, project) = project(&on_disk);
        assert!(dir.path().join(&secret).is_file());
        let files = NamedFiles::default();
        assert!(!is_stored_as_written(&secret));
        assert!(
            files
                .keep(
                    &project,
                    &secret,
                    &Observed {
                        paths: names.clone(),
                        deletes: 0
                    },
                    7
                )
                .is_none(),
            "a session id the Kernel would rewrite records nothing"
        );
        let mut paths = vec!["docs/absent.md".to_owned(), secret];
        paths.extend(names.iter().cloned());
        let observed = Observed { paths, deletes: 0 };
        let taken: Vec<_> = (0..4)
            .map(|_| Arc::clone(&files.slots).try_acquire_owned().unwrap())
            .collect();
        assert!(Arc::clone(&files.slots).try_acquire_owned().is_err());
        assert!(files.keep(&project, "s", &observed, 7).is_none());
        drop(taken);
        let record = files
            .keep(&project, "s", &observed, 7)
            .expect("a slot is free");
        assert_eq!(record.paths, names[..8]);
        assert_eq!(record.used_ms, 7);
        assert_eq!(files.slots.available_permits(), 3);
        assert!(files.memo().sessions.is_empty());
        drop(record);
        assert_eq!(files.slots.available_permits(), 4);

        // A session with two places left hands two paths.
        let session_owner = NamedFiles::owner(&project, "s");
        for index in 0..62 {
            files
                .memo()
                .record(&session_owner, &format!("src/{index}.rs"));
        }
        let record = files.keep(&project, "s", &observed, 7).unwrap();
        assert_eq!(record.paths, names[..2]);
        drop(record);
        // A path another pass recorded since the observation takes none of
        // the places.
        files.memo().record(&session_owner, &names[0]);
        let record = files.keep(&project, "s", &observed, 7).unwrap();
        assert_eq!(record.paths, names[1..2]);
    }

    /// The packer charges every `NAMED_FILES` line for one fixed clause.
    /// Under the estimator the budget uses, a line with any clause the
    /// renderer writes costs at most the same line with the charged clause:
    /// a commit time and the same time on disk, for minutes spread over the
    /// years 0000 to 9999, a time outside those years in both forms, and
    /// the clause of a history the read did not follow.
    #[test]
    fn a_clause_costs_at_most_its_charge() {
        use changes::LastChange;
        let tokens = |line: &str| crate::token_cache::cached_estimate_tokens(&format!("{line}\n"));
        // The first minute of the year 0000 and the minute after the last
        // of the year 9999.
        let (first_ms, end_ms) = (-62_167_219_200_000, 253_402_300_800_000);
        assert_eq!(minute(first_ms), "0000-01-01T00:00Z");
        assert_eq!(minute(end_ms - 1), "9999-12-31T23:59Z");
        // A prime number of minutes between samples walks every minute,
        // hour, day, and month digit: 100,003 minutes over the years 0000
        // to 9999, and 20,011 over the three centuries from 1970.
        for (prefix, years, step_minutes) in [
            (
                "docs/guide.md (the agent used it in 1 earlier session, last on 2026-03-01",
                first_ms..end_ms,
                100_003,
            ),
            (
                "tools/lint/check_style.sh (the agent used it in 12 earlier sessions, last on 1999-12-31",
                0..9_500_000_000_000,
                20_011,
            ),
        ] {
            let charge = tokens(&format!("{prefix}{CHARGED_CLAUSE}"));
            let costs = |change: LastChange| {
                tokens(&format!(
                    "{prefix}{LAST_CHANGE_CLAUSE}{})",
                    last_change(change)
                ))
            };
            for undated in [
                LastChange::HistoryUnread,
                LastChange::Committed(first_ms - 1),
                LastChange::OnDisk(first_ms - 1),
                LastChange::Committed(end_ms),
                LastChange::OnDisk(end_ms),
                LastChange::OnDisk(i64::MIN),
                LastChange::OnDisk(i64::MAX),
            ] {
                assert!(costs(undated) <= charge, "{undated:?}");
            }
            let step = step_minutes * 60_000;
            let mut at_ms = years.start;
            while at_ms < years.end {
                for change in [LastChange::Committed(at_ms), LastChange::OnDisk(at_ms)] {
                    assert!(
                        costs(change) <= charge,
                        "{change:?}: {} tokens against a charge of {charge}",
                        costs(change)
                    );
                }
                at_ms += step;
            }
        }
        assert_eq!(
            revised_part(&format!("docs/a.md (used{CHARGED_CLAUSE}")),
            "docs/a.md (used"
        );
    }

    /// A line's last-change clause is the UTC minute of the commit that
    /// gave the file its bytes, the same form with `on disk` for a time the
    /// file system holds, and words with no time for a history the read
    /// did not follow to that commit and for a time outside the years 0000
    /// to 9999.
    #[test]
    fn a_last_change_clause_has_three_forms() {
        use changes::LastChange;
        // 2026-03-01T09:30:59.999Z.
        let at_ms = 1_772_357_459_999;
        assert_eq!(
            last_change(LastChange::Committed(at_ms)),
            "2026-03-01T09:30Z"
        );
        assert_eq!(
            last_change(LastChange::OnDisk(at_ms)),
            "2026-03-01T09:30Z on disk"
        );
        assert_eq!(
            last_change(LastChange::HistoryUnread),
            "at a time the read history does not show"
        );
        // The minute after 9999-12-31T23:59Z, and a time no calendar holds.
        assert_eq!(
            last_change(LastChange::Committed(253_402_300_800_000)),
            "at an unknown time"
        );
        assert_eq!(
            last_change(LastChange::OnDisk(i64::MAX)),
            "at an unknown time on disk"
        );
    }
}

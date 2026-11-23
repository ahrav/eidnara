//! Project files a user's message named and the agent then used.
//!
//! A message the user sent names a file of the project, and a tool call of
//! the same turn then reads the file or passes it to a command. A later
//! session is delivered the file's path, in the `NAMED_FILES` element of its
//! `<project-memory>` block, while the file exists. A record holds the path,
//! the session, and the time of the pass that showed the use, and holds none
//! of the file's content.
//!
//! Each admitted transform pass of a primary Pi session at the project root
//! shows the turn in progress: the last message of the user and the tool
//! calls after it. A message of the user is a row that Pi gives the user
//! role and that carries neither the synthetic nor the summary mark: a
//! prompt the user types, a template the user invokes, or a prompt an
//! extension sends in the user's name.
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

/// The domain of the use records. The daemon alone writes and changes its
/// rows: `kernel.commit` refuses an operation that names the domain, one of
/// its rows, or a reserved id ([`is_reserved_id`]), and artifact ingest
/// refuses an artifact that names the domain or a reserved id. The daemon's memory reads select
/// the `memory` domain and the search projection publishes none of these
/// rows, so the capture extractor and `eidnara_search` see none of them.
pub(crate) const DOMAIN_ID: &str = "named-files";
/// The prefixes of a record's object id and decision id.
const ID_PREFIXES: [&str; 2] = ["nfu_", "dnf_"];
const SOURCE_ID: &str = "named_file_use";
/// The recorder's own source kind. `kernel.commit` derives a caller's
/// classes from a closed list of source kinds that holds no such value, and
/// artifact ingest refuses it, so a row that carries it was written here.
pub(crate) const SOURCE_KIND: &str = "named_file_recorder";

/// Whether `id` is an id the recorder derives: a record's object id or
/// decision id, or the object id of its domain. `kernel.commit` and artifact
/// ingest refuse a caller's row under one, which would keep the recorder's
/// own write of that id from committing.
pub(crate) fn is_reserved_id(id: &str) -> bool {
    id.strip_prefix(kernel_routes::commit::DOMAIN_OBJECT_PREFIX) == Some(DOMAIN_ID)
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
/// and [`MAX_PASS_FILES`] paths.
pub(crate) const RETAINED_BYTES_BOUND: u64 =
    (MEMO_SESSIONS as u64) * 4 * 1024 + (WRITE_SLOTS as u64) * 16 * 1024;

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
/// it. `raw` holds path characters alone, so a root spelled with another
/// character admits no absolute path, and nothing is normalized: a `.` or
/// `..` component past the leading marks names no file here.
fn project_path<'a>(root: &Path, raw: &'a str) -> Option<&'a str> {
    if !raw.chars().all(is_path_char) {
        return None;
    }
    let rest = if raw.starts_with('/') {
        raw.strip_prefix(root.to_str()?)?.strip_prefix('/')?
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
/// its own text runs ([`own_text`]). A path word is a run of path characters
/// that holds a `.` or a `/` once the periods that end it are dropped. Each
/// of the first [`MAX_NAMES_CHECKED`] distinct path words names a path as
/// written and, for a word that opens with Pi's file-reference mark `@`,
/// without the mark.
fn named_paths(root: &Path, message: &crate::wire::IngressMessage) -> Vec<String> {
    let parts: Vec<&str> = message
        .ck
        .content()
        .iter()
        .filter_map(|block| match block.kind() {
            BlockKind::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    if parts.iter().map(|part| part.len()).sum::<usize>() > MAX_MESSAGE_BYTES {
        return Vec::new();
    }
    let path_words = parts.iter().flat_map(|part| {
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
        for spelling in std::iter::once(word).chain(word.strip_prefix('@')) {
            if let Some(path) = project_path(root, spelling)
                && !named.iter().any(|held| held == path)
            {
                named.push(path.to_owned());
            }
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

/// The files of `named` that the calls of `messages` used. A call of Pi's
/// `read` uses the file its path argument names, which Pi opens without
/// one leading `@`; a call of Pi's `bash` uses the files that the first
/// [`MAX_COMMAND_WORDS`] runs of path characters with a `.` or a `/` in its
/// command line name as written.
/// A call counts when its id is the id of no other call of `messages` and
/// of one result, which follows the call and is text, JSON, or content that
/// carries no error.
fn used_by_calls(
    root: &Path,
    messages: &[Arc<crate::wire::IngressMessage>],
    named: &[String],
) -> Vec<String> {
    let blocks = || {
        messages
            .iter()
            .flat_map(|message| message.ck.content())
            .map(|block| block.kind())
    };
    let mut ids: HashMap<&str, Answers> = HashMap::new();
    for block in blocks() {
        match block {
            BlockKind::ToolCall { id, .. } => ids.entry(id.as_str()).or_default().calls += 1,
            BlockKind::ToolResult { id, output, .. } => {
                let answers = ids.entry(id.as_str()).or_default();
                answers.result_first |= answers.calls == 0;
                answers.results += 1;
                answers.succeeded = matches!(
                    output.kind,
                    OutputKind::Text { .. } | OutputKind::Json { .. } | OutputKind::Content { .. }
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
    for block in blocks() {
        let BlockKind::ToolCall {
            id, name, input, ..
        } = block
        else {
            continue;
        };
        if !ids
            .get(id.as_str())
            .is_some_and(Answers::is_one_answered_call)
        {
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
    /// recorded. Returns how many it wrote.
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
        kernel_routes::commit::ensure_domain(envelope, DOMAIN_ID, &mut domains)?;
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
    /// would rewrite the record's rationale, when every write slot is taken, and
    /// when a session was deleted since the pass was observed. The caller
    /// runs on the blocking pool: the files are looked for.
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
        let mut paths = used_by_calls(project.root(), calls, &named);
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
/// used the file, and the day of the last use. The read holds the newest
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
    let lines = ordered
        .into_iter()
        .take(MAX_CHECKED)
        .filter(|(path, _)| is_recordable(path) && is_project_file(root.as_fd(), path))
        .take(MAX_DELIVERED)
        .map(|(path, file)| CanonicalMemory {
            object_id: file.id,
            category: NAMED_FILES_SECTION.to_owned(),
            content: format!(
                "{path} (the agent used it in {} earlier session{}, last on {})",
                file.sessions.len(),
                if file.sessions.len() == 1 { "" } else { "s" },
                day(file.last_ms)
            ),
        })
        .collect();
    Ok((lines, read.truncated))
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

    /// A call of `name` under `id` and the result the tool answered it with.
    fn answered_as(
        id: &str,
        name: &str,
        input: Value,
        output: OutputKind,
    ) -> Arc<crate::wire::IngressMessage> {
        Arc::new(message(
            "assistant",
            vec![
                BlockKind::ToolCall {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    input,
                    provider_executed: false,
                },
                BlockKind::ToolResult {
                    id: id.to_owned(),
                    tool_name: name.to_owned(),
                    output: memory_store::ToolOutput::bare(output),
                    provider_executed: false,
                },
            ],
        ))
    }

    fn answered(name: &str, input: Value, output: OutputKind) -> Arc<crate::wire::IngressMessage> {
        answered_as("call-1", name, input, output)
    }

    fn call(name: &str, input: Value) -> Arc<crate::wire::IngressMessage> {
        answered(name, input, OutputKind::Text { text: "ok".into() })
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
        // Two-byte characters: 65,537 bytes in 32,773 characters.
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
    }

    /// A word that opens with Pi's `@` names the path as written and the
    /// path without the mark. Pi's `read` opens its path argument without
    /// one leading `@`, and a command line names a path as written, periods
    /// included.
    #[test]
    fn a_pi_file_reference_names_the_file_pi_opens() {
        let named = named_paths(root(), &user("See @docs/a.md and @scope/pkg.json."));
        assert_eq!(
            named,
            [
                "@docs/a.md",
                "docs/a.md",
                "@scope/pkg.json",
                "scope/pkg.json"
            ]
        );
        let used =
            |message: Arc<crate::wire::IngressMessage>| used_by_calls(root(), &[message], &named);
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
        assert!(used(call("bash", json!({"command": "cat docs/a.md."}))).is_empty());
        assert!(used(call("bash", json!({"command": "cat x/../docs/a.md"}))).is_empty());
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
        assert_eq!(
            used_by_calls(root(), &[command(255)], &named),
            ["docs/a.md"]
        );
        assert!(used_by_calls(root(), &[command(256)], &named).is_empty());
        assert_eq!(
            used_by_calls(
                root(),
                &[
                    answered_as(
                        "call-1",
                        "read",
                        json!({"path": "docs/b.md"}),
                        OutputKind::Json { value: json!({}) }
                    ),
                    answered_as(
                        "call-2",
                        "read",
                        json!({"path": "./docs/a.md", "filePath": "docs/c.md"}),
                        OutputKind::Content { blocks: Vec::new() }
                    ),
                ],
                &named
            ),
            ["docs/b.md", "docs/a.md"]
        );
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
            Arc::new(message(
                "assistant",
                vec![BlockKind::ToolCall {
                    id: "call-9".to_owned(),
                    name: "read".to_owned(),
                    input: json!({"path": "docs/a.md"}),
                    provider_executed: false,
                }],
            )),
        ] {
            assert!(
                used_by_calls(root(), std::slice::from_ref(&unused), &named).is_empty(),
                "{unused:?}"
            );
        }
    }

    /// A call counts when its id is one call's and one result's, and the
    /// result follows the call. Two calls under one id, a call with two
    /// results, and a call whose id an earlier result carries use nothing:
    /// the rows do not say which result answers which call.
    #[test]
    fn a_call_counts_when_its_id_pairs_it_with_one_later_result() {
        let named = ["docs/a.md".to_owned(), "docs/b.md".to_owned()];
        let block = |kind: BlockKind| Arc::new(message("assistant", vec![kind]));
        let read = |id: &str, path: &str| {
            block(BlockKind::ToolCall {
                id: id.to_owned(),
                name: "read".to_owned(),
                input: json!({ "path": path }),
                provider_executed: false,
            })
        };
        let result = |id: &str, output: OutputKind| {
            block(BlockKind::ToolResult {
                id: id.to_owned(),
                tool_name: "read".to_owned(),
                output: memory_store::ToolOutput::bare(output),
                provider_executed: false,
            })
        };
        let ok = |id: &str| result(id, OutputKind::Text { text: "ok".into() });
        let failed = |id: &str| {
            result(
                id,
                OutputKind::ErrorText {
                    text: "no such file".into(),
                },
            )
        };
        let used = |rows: &[Arc<crate::wire::IngressMessage>]| used_by_calls(root(), rows, &named);
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
}

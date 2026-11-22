//! Project files a user's message named and the agent then used.
//!
//! A message the user sent names a file of the project, and a tool call of
//! the same turn then reads the file or passes it to a command. A later
//! session is delivered the file's path, in the `NAMED_FILES` element of its
//! `<project-memory>` block, while the file exists. A record holds the path,
//! the session, and the time of the pass that showed the use, and holds none
//! of the file's content.
//!
//! Each admitted transform pass of a primary Pi session shows the turn in
//! progress: the last message the user typed and the tool calls after it.
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
/// its rows, or an id under [`ID_PREFIXES`]. The daemon's memory reads select
/// the `memory` domain and the search projection publishes none of these
/// rows, so the capture extractor and `eidnara_search` see none of them.
pub(crate) const DOMAIN_ID: &str = "named-files";
/// The object id the Kernel gives the domain's own row.
pub(crate) const DOMAIN_OBJECT_ID: &str = "domain:named-files";
/// The prefixes of a record's object id and decision id.
pub(crate) const ID_PREFIXES: [&str; 2] = ["nfu_", "dnf_"];
const SOURCE_ID: &str = "named_file_use";
/// The recorder's own source kind. `kernel.commit` derives a caller's
/// classes from a closed list of source kinds that holds no such value, so a
/// row that carries it was written here.
const SOURCE_KIND: &str = "named_file_recorder";
const DECISION_KIND: &str = "named_file_use";

/// Paths one pass hands to a record.
const MAX_PASS_FILES: usize = 8;
/// Distinct path words of one user message that are matched against calls.
const MAX_NAMES_CHECKED: usize = 32;
/// Bytes of one user message that are read for path words.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// A recorded path, in bytes.
const MAX_PATH_BYTES: usize = 256;
/// A path word or path argument, in bytes, before it is resolved.
const MAX_WORD_BYTES: usize = 4 * 1024;
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
        MAX_SESSION_FILES - self.sessions.get(owner).map_or(0, HashSet::len)
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

/// What one submitted window shows: the project paths that a message the
/// user typed names and that a tool call of its turn used, which the session
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
/// `._-/+@`, which the confined-path check admits.
fn is_recordable(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_BYTES
        && path.chars().all(is_path_char)
        && crate::confined_path::check(path).is_ok()
}

/// The session's working directory, as its components beneath the project
/// root.
struct Base<'a> {
    root: &'a Path,
    components: Vec<&'a str>,
}

impl<'a> Base<'a> {
    /// `None` for a working directory that is no recordable path beneath
    /// the root; the empty directory is the root.
    fn new(root: &'a Path, working_directory: &'a str) -> Option<Self> {
        let components = if working_directory.is_empty() {
            Vec::new()
        } else if is_recordable(working_directory) {
            working_directory.split('/').collect()
        } else {
            return None;
        };
        Some(Self { root, components })
    }

    /// `raw` as a recordable path beneath the project root. A relative path
    /// is read from the working directory, where Pi resolves it; an
    /// absolute path beneath the root is taken relative to the root. `.`
    /// and `..` components are resolved, and a path that leaves the root
    /// names no file.
    fn resolve(&self, raw: &str) -> Option<String> {
        if raw.len() > MAX_WORD_BYTES || !raw.chars().all(is_path_char) {
            return None;
        }
        let (mut components, rest) = match raw.strip_prefix('/') {
            Some(_) => (
                Vec::new(),
                Path::new(raw).strip_prefix(self.root).ok()?.to_str()?,
            ),
            None => (self.components.clone(), raw),
        };
        for component in rest.split('/') {
            match component {
                "" | "." => {}
                ".." => {
                    components.pop()?;
                }
                name => components.push(name),
            }
        }
        let bytes = components.iter().map(|name| name.len()).sum::<usize>() + components.len();
        if bytes > MAX_PATH_BYTES + 1 {
            return None;
        }
        let path = components.join("/");
        is_recordable(&path).then_some(path)
    }
}

/// The runs of path characters in `text` that hold a `.` or a `/` once the
/// periods that end them are dropped.
fn path_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|ch| !is_path_char(ch))
        .map(|word| word.trim_end_matches('.'))
        .filter(|word| word.contains(['.', '/']))
}

/// The text parts of `message`, joined by line breaks as Pi joins them, cut
/// at [`MAX_MESSAGE_BYTES`]. A run of path characters that the cut divides
/// is left out whole, so the text holds no shortened path.
fn message_text(message: &crate::wire::IngressMessage) -> String {
    let mut text = String::new();
    for block in message.ck.content() {
        let BlockKind::Text { text: part } = block.kind() else {
            continue;
        };
        if !text.is_empty() {
            text.push('\n');
        }
        let room = MAX_MESSAGE_BYTES.saturating_sub(text.len());
        if part.len() <= room {
            text.push_str(part);
            continue;
        }
        let cut = part.floor_char_boundary(room);
        let mut kept = &part[..cut];
        if part[cut..].starts_with(is_path_char) {
            kept = kept.trim_end_matches(is_path_char);
        }
        text.push_str(kept);
        break;
    }
    text
}

/// The lines of `text` that the message's sender wrote. The lines of a code
/// fence are the sender's, tag lines among them. Outside a fence, a line
/// that holds one opening tag alone opens a block another writer generated,
/// such as a hint a plugin appends: the block runs through the last line of
/// the text that closes the tag, or to the end of the text when no line
/// does. A line that holds a closing tag alone is left out as well. Capture
/// reads a message's fences and tag blocks the same way.
fn own_lines(text: &str) -> Vec<&str> {
    use crate::memory_capture::{fence_opener, tag_name};
    let lines: Vec<&str> = text.lines().collect();
    let mut last_closer: HashMap<&str, usize> = HashMap::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some((name, true)) = tag_name(line) {
            last_closer.insert(name, index);
        }
    }
    let mut kept = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        if let Some(fence) = fence_opener(line) {
            let end = (index + 1..lines.len())
                .find(|&next| {
                    let next = lines[next].trim();
                    next.len() >= fence.len() && next.chars().all(|ch| fence.starts_with(ch))
                })
                .unwrap_or(lines.len() - 1);
            kept.extend_from_slice(&lines[index..=end]);
            index = end + 1;
        } else if let Some((name, closing)) = tag_name(line) {
            index = match last_closer
                .get(name)
                .filter(|closer| !closing && **closer > index)
            {
                Some(closer) => closer + 1,
                None if closing => index + 1,
                None => lines.len(),
            };
        } else {
            kept.push(lines[index]);
            index += 1;
        }
    }
    kept
}

/// The project paths a message names: for each of the first
/// [`MAX_NAMES_CHECKED`] distinct path words of the lines its sender wrote,
/// the word as written and, for a word that opens with Pi's file-reference
/// mark `@`, the word without it.
fn named_paths(base: &Base<'_>, message: &crate::wire::IngressMessage) -> Vec<String> {
    let text = message_text(message);
    let mut seen: HashSet<&str> = HashSet::new();
    let mut named = Vec::new();
    for word in own_lines(&text).into_iter().flat_map(path_words) {
        if seen.len() == MAX_NAMES_CHECKED {
            break;
        }
        if !seen.insert(word) {
            continue;
        }
        for spelling in std::iter::once(word).chain(word.strip_prefix('@')) {
            if let Some(path) = base.resolve(spelling)
                && !named.contains(&path)
            {
                named.push(path);
            }
        }
    }
    named
}

/// The files of `named` that the calls of `messages` used. A call of Pi's
/// `read` uses the file its path argument names, which Pi opens without
/// one leading `@`; a call of Pi's `bash` uses the files its command line's
/// first [`MAX_COMMAND_WORDS`] path words name as written. A call counts
/// when `messages` hold its result and the result is no error.
fn used_by_calls(
    base: &Base<'_>,
    messages: &[Arc<crate::wire::IngressMessage>],
    named: &[String],
) -> Vec<String> {
    let blocks = || {
        messages
            .iter()
            .flat_map(|message| message.ck.content())
            .map(|block| block.kind())
    };
    let answered: HashSet<&str> = blocks()
        .filter_map(|block| match block {
            BlockKind::ToolResult { id, output, .. }
                if !matches!(
                    output.kind,
                    OutputKind::ErrorText { .. }
                        | OutputKind::ErrorJson { .. }
                        | OutputKind::ErrorContent { .. }
                        | OutputKind::ExecutionDenied { .. }
                ) =>
            {
                Some(id.as_str())
            }
            _ => None,
        })
        .collect();
    let named_file = |raw: &str| {
        let path = base.resolve(raw)?;
        named.iter().find(|file| **file == path)
    };
    let mut used: Vec<&String> = Vec::new();
    for block in blocks() {
        let BlockKind::ToolCall {
            id, name, input, ..
        } = block
        else {
            continue;
        };
        if !answered.contains(id.as_str()) {
            continue;
        }
        let files: Vec<&String> = match tool_vocabulary::pi_operand(name, input) {
            Some(PiOperand::Read(path)) => named_file(path.strip_prefix('@').unwrap_or(path))
                .into_iter()
                .collect(),
            Some(PiOperand::Command(line)) => path_words(line)
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
            source_kind: SOURCE_KIND.into(),
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
        let room = {
            let memo = self.memo();
            if memo.deletes != observed.deletes {
                return None;
            }
            memo.room(&owner).min(MAX_PASS_FILES)
        };
        let root = open_root(project.root())?;
        let paths: Vec<String> = observed
            .paths
            .iter()
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
    /// configuration permits memory capture shows: the project paths that
    /// one of the last two messages the user typed names and that a call of
    /// that message's turn used, except the paths the session has recorded.
    /// A message's turn is the rows after it, up to the next message the
    /// user typed. The session's working directory comes with the request;
    /// a request without one, or with one that is no path beneath the
    /// project root, shows nothing. Reads the window and the memo and
    /// changes nothing.
    pub(crate) fn observe_named_files(
        &self,
        binding: &SessionBinding,
        request: &crate::transform::TransformRequest,
    ) -> Option<Observed> {
        if request.is_subagent
            || binding.harness != "pi"
            || !crate::memory_capture::capture_enabled(binding)
        {
            return None;
        }
        let native = request.native_messages.as_deref().filter(|rows| {
            request.serializer_profile == "pi" && rows.len() == request.messages.len()
        })?;
        let project = &binding.kernel_project;
        let base = Base::new(project.root(), request.working_directory.as_deref()?)?;
        // Read before the window, so a deletion that lands while the pass
        // runs is seen when the record is taken and when it is written.
        let deletes = self.named_files.memo().deletes;
        let typed = |index: &usize| {
            let message = &request.messages[*index].ck;
            message.role == "user"
                && native[*index]
                    .pointer("/message/role")
                    .and_then(Value::as_str)
                    == Some("user")
                && !message.meta.synthetic
                && !message.meta.summary
        };
        let mut openers = (0..request.messages.len()).rev().filter(typed);
        let last = openers.next()?;
        let turns = [
            openers.next().map(|earlier| (earlier, last)),
            Some((last, request.messages.len())),
        ];
        let mut paths: Vec<String> = Vec::new();
        for (opener, end) in turns.into_iter().flatten() {
            let calls = &request.messages[opener + 1..end];
            if calls.is_empty() {
                continue;
            }
            let named = named_paths(&base, &request.messages[opener]);
            if named.is_empty() {
                continue;
            }
            for path in used_by_calls(&base, calls, &named) {
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
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

    /// A call of `name` and the result the tool answered it with.
    fn answered(name: &str, input: Value, output: OutputKind) -> Arc<crate::wire::IngressMessage> {
        Arc::new(message(
            "assistant",
            vec![
                BlockKind::ToolCall {
                    id: "call-1".to_owned(),
                    name: name.to_owned(),
                    input,
                    provider_executed: false,
                },
                BlockKind::ToolResult {
                    id: "call-1".to_owned(),
                    tool_name: name.to_owned(),
                    output: memory_store::ToolOutput::bare(output),
                    provider_executed: false,
                },
            ],
        ))
    }

    fn call(name: &str, input: Value) -> Arc<crate::wire::IngressMessage> {
        answered(name, input, OutputKind::Text { text: "ok".into() })
    }

    const ROOT: &str = "/work/project";

    fn at_root() -> Base<'static> {
        Base::new(Path::new(ROOT), "").unwrap()
    }

    /// A path is held relative to the project root. A relative path is read
    /// from the session's working directory, an absolute path beneath the
    /// root is taken relative to it, and `.` and `..` are resolved. The
    /// resolved path is at most 256 bytes, whatever the length of the word
    /// that names it, and quotation marks are no part of a path.
    #[test]
    fn a_path_is_resolved_from_the_working_directory_within_256_bytes() {
        let base = at_root();
        let resolved = |raw: &str| base.resolve(raw);
        assert_eq!(resolved("docs/a.md").as_deref(), Some("docs/a.md"));
        assert_eq!(resolved("./docs/./a.md").as_deref(), Some("docs/a.md"));
        assert_eq!(resolved("docs/../README.md").as_deref(), Some("README.md"));
        assert_eq!(
            resolved("/work/project/docs/a.md").as_deref(),
            Some("docs/a.md")
        );
        assert_eq!(resolved("/work/other/docs/a.md"), None);
        assert_eq!(resolved("../project/docs/a.md"), None);
        assert_eq!(resolved(".git/config"), None);
        assert_eq!(resolved("docs/a b.md"), None);
        assert_eq!(resolved("\"docs/a.md\""), None);
        assert_eq!(resolved("."), None);

        let nested = Base::new(Path::new(ROOT), "packages/app").unwrap();
        assert_eq!(
            nested.resolve("README.md").as_deref(),
            Some("packages/app/README.md")
        );
        assert_eq!(
            nested.resolve("../../README.md").as_deref(),
            Some("README.md")
        );
        assert_eq!(nested.resolve("../../../README.md"), None);
        assert_eq!(
            nested.resolve("/work/project/README.md").as_deref(),
            Some("README.md")
        );
        for refused in ["../elsewhere", "/work/project", "docs/a b", ".git"] {
            assert!(Base::new(Path::new(ROOT), refused).is_none(), "{refused}");
        }
        assert!(Base::new(Path::new(ROOT), &"d/".repeat(128)).is_none());

        let longest = format!("{}/{}.md", "a".repeat(100), "b".repeat(152));
        assert_eq!(longest.len(), 256);
        assert_eq!(resolved(&longest), Some(longest.clone()));
        assert_eq!(resolved(&format!("{longest}x")), None);
        // An absolute spelling of 270 bytes resolves to the 256-byte path,
        // and a relative one resolves against the directory's own bytes.
        assert_eq!(
            resolved(&format!("{ROOT}/{longest}")),
            Some(longest.clone())
        );
        let in_d = Base::new(Path::new(ROOT), "d").unwrap();
        assert_eq!(
            in_d.resolve(&longest[..254]).map(|path| path.len()),
            Some(256)
        );
        assert_eq!(in_d.resolve(&longest[..255]), None);
        assert_eq!(resolved(&"a/".repeat(MAX_WORD_BYTES)), None);
    }

    /// A message's path words are its runs of path characters that hold a
    /// `.` or a `/` once their closing periods are dropped. The first 32
    /// distinct words are read, whatever they resolve to; a repeated word
    /// takes no second place.
    #[test]
    fn the_first_32_distinct_path_words_of_a_message_are_read() {
        assert_eq!(
            path_words("Read docs/a.md. Then run `tools/check.sh`, twice.").collect::<Vec<_>>(),
            ["docs/a.md", "tools/check.sh"]
        );
        assert_eq!(
            path_words("Done. Fine... v1.2").collect::<Vec<_>>(),
            ["v1.2"]
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
            named_paths(&at_root(), &user(&text)),
            ["docs/a.md", "docs/b.md"]
        );
    }

    /// The words come from the lines the sender wrote. A block that a
    /// tag-only line opens runs through the last line that closes the tag,
    /// or to the end of the text; a code fence is the sender's, a tag line
    /// inside it too; and the parts of a message are read as one text.
    #[test]
    fn path_words_come_from_the_senders_own_lines() {
        let text = "Follow docs/a.md.\n```\n<note>\ntools/check.sh\n```\n\
                    <context-hint>\nSee docs/hinted.md.\n</context-hint>\n\
                    docs/between.md\n</context-hint>\n\
                    Then docs/b.md.\n</stray>\n<unclosed>\ndocs/after.md";
        assert_eq!(
            named_paths(&at_root(), &user(text)),
            ["docs/a.md", "tools/check.sh", "docs/b.md"]
        );
        // A block another writer generated, split over two text parts.
        let parts = message(
            "user",
            vec![
                BlockKind::Text {
                    text: "Follow docs/a.md.\n<context-hint>".into(),
                },
                BlockKind::Text {
                    text: "See docs/hinted.md.\n</context-hint>\nThen docs/b.md.".into(),
                },
            ],
        );
        assert_eq!(named_paths(&at_root(), &parts), ["docs/a.md", "docs/b.md"]);
    }

    /// A message is read through its first 64 KiB, and a path word the
    /// bound divides is left out whole.
    #[test]
    fn a_path_word_the_message_bound_divides_is_left_out() {
        let filler = "x ".repeat(MAX_MESSAGE_BYTES / 2 - 8);
        assert_eq!(filler.len(), MAX_MESSAGE_BYTES - 16);
        // The bound falls inside `docs/divided.md`.
        let divided = format!("docs/a.md {filler}docs/divided.md docs/past.md");
        assert_eq!(named_paths(&at_root(), &user(&divided)), ["docs/a.md"]);
        // The bound falls right after `docs/whole.md`, before a space.
        let whole = format!("{filler}   docs/whole.md docs/past.md");
        assert_eq!(
            &whole[..MAX_MESSAGE_BYTES][MAX_MESSAGE_BYTES - 13..],
            "docs/whole.md"
        );
        assert_eq!(named_paths(&at_root(), &user(&whole)), ["docs/whole.md"]);
    }

    /// A word that opens with Pi's `@` names the path as written and the
    /// path without the mark. Pi's `read` opens its path argument without
    /// one leading `@`, and a command line names a path as written.
    #[test]
    fn a_pi_file_reference_names_the_file_pi_opens() {
        let base = at_root();
        let named = named_paths(&base, &user("See @docs/a.md and @scope/pkg.json."));
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
            |message: Arc<crate::wire::IngressMessage>| used_by_calls(&base, &[message], &named);
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
    }

    /// Pi's `read` uses the file its path names, and Pi's `bash` the files
    /// the first 256 path words of its command line name. A call of another
    /// tool, a call whose result is an error, and a call with no result in
    /// the window use nothing.
    #[test]
    fn a_read_or_a_command_that_was_answered_uses_a_file() {
        let base = at_root();
        let named = ["docs/a.md".to_owned(), "docs/b.md".to_owned()];
        let command = |filler: usize| {
            let words: Vec<String> = (0..filler).map(|index| format!("f{index}.txt")).collect();
            call(
                "bash",
                json!({"command": format!("cat {} docs/a.md", words.join(" "))}),
            )
        };
        assert_eq!(used_by_calls(&base, &[command(255)], &named), ["docs/a.md"]);
        assert!(used_by_calls(&base, &[command(256)], &named).is_empty());
        assert_eq!(
            used_by_calls(
                &base,
                &[
                    call("read", json!({"path": "docs/b.md"})),
                    call(
                        "read",
                        json!({"path": "docs/a.md", "filePath": "docs/c.md"})
                    ),
                ],
                &named
            ),
            ["docs/b.md", "docs/a.md"]
        );
        for unused in [
            call(
                "edit",
                json!({"path": "docs/a.md", "oldText": "a", "newText": "b"}),
            ),
            call("ls", json!({"path": "docs/a.md"})),
            call("read", json!({"filePath": "docs/a.md"})),
            answered(
                "read",
                json!({"path": "docs/a.md"}),
                OutputKind::ErrorText {
                    text: "no such file".into(),
                },
            ),
            answered(
                "bash",
                json!({"command": "cat docs/a.md"}),
                OutputKind::ExecutionDenied { reason: None },
            ),
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
                used_by_calls(&base, std::slice::from_ref(&unused), &named).is_empty(),
                "{unused:?}"
            );
        }
        let nested = Base::new(Path::new(ROOT), "docs").unwrap();
        assert!(
            used_by_calls(
                &nested,
                &[call("read", json!({"path": "docs/a.md"}))],
                &named
            )
            .is_empty(),
            "the argument is read from the working directory"
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
        let (_dir, project) = project(&on_disk);
        let files = NamedFiles::default();
        let secret = "keys/sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678.txt".to_owned();
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
    }
}

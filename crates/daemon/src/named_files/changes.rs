//! When a project file last changed, for the lines of the `NAMED_FILES`
//! element.
//!
//! A file whose bytes are the blob HEAD holds for its path last changed
//! with the newest commit after which that blob stayed. The history is
//! read from HEAD through the first parent that holds the same blob, as
//! `git log` simplifies the history of one file, and the commit none of
//! whose parents holds the blob is the change. Its committer time is the
//! time the change entered this history: a clone or a checkout writes the
//! file anew and leaves that time as it is. A history the read does not
//! follow to that commit gives no time. Every other file shows its
//! modification time on disk: a file HEAD holds no blob for, a file whose
//! bytes differ from that blob, a file past [`MAX_HASHED_BYTES`], and a file
//! of a project whose root opens as no Git work tree or whose HEAD names no
//! readable commit.
//!
//! A pass reads the file and no repository. One thread, the reader, reads
//! the repository for every pass, and a pass waits at most [`WAIT`] for its
//! answer, so a repository that blocks or spins a read costs a pass that
//! wait and the line its commit time. The reader takes HEAD through
//! [`crate::memory_capture::observe_head`] and each commit and tree through
//! the attestation verifier's object reader, which bounds an object's size
//! and the delta chains of the repository's own packs and checks that the
//! bytes hash to the id. An object the repository holds only through an
//! alternate is read by gix's store, whose delta resolution has the
//! allocation bound alone.

use std::collections::{HashMap, VecDeque};
use std::io::Read as _;
use std::os::fd::BorrowedFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::attestation_verifier::{ALLOCATION_LIMIT, INDEX_SLOTS, ObjectClass, Objects};
use crate::memory_capture::{FrozenHead, observe_head};

/// When a file last changed, as one delivery reads it. A time is in
/// milliseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LastChange {
    /// The committer time of the commit that gave the file its bytes.
    Committed(i64),
    /// The file's bytes are the blob HEAD holds, and the read history does
    /// not reach the commit that gave the file those bytes: the walk met
    /// its bound, its time budget, or a commit or tree it could not read.
    HistoryUnread,
    /// The modification time of the file on disk.
    OnDisk(i64),
}

/// Parents one file's walk reads.
const MAX_PARENTS_READ: usize = 256;
/// The largest file whose bytes are hashed against the blob HEAD holds.
const MAX_HASHED_BYTES: u64 = 256 * 1024;
/// How long a pass waits for the reader's answer.
const WAIT: Duration = Duration::from_secs(1);
/// How long the reader works on one pass's files. The reader checks the
/// budget before each object it reads.
const JOB_BUDGET: Duration = Duration::from_secs(2);
/// Passes whose files wait for the reader at once. A pass that finds the
/// queue full asks nothing.
const QUEUE: usize = 4;
/// Project roots whose answers the memo holds.
const MEMO_ROOTS: usize = 64;
/// Paths of one root whose answers the memo holds.
const MEMO_FILES: usize = 64;

/// The resident bytes the memo and the queue hold at their bounds: for each
/// of [`MEMO_ROOTS`] roots its path and, for each of [`MEMO_FILES`] files, a
/// path of at most 256 bytes, a blob id, and a time; for each of [`QUEUE`]
/// waiting passes and the one the reader works on, a root path and eight
/// files.
pub(crate) const RETAINED_BYTES_BOUND: u64 =
    (MEMO_ROOTS as u64) * (8 * 1024 + (MEMO_FILES as u64) * 512) + (QUEUE as u64 + 1) * 16 * 1024;

/// What the reader found for one file: the path and the blob a pass read on
/// disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    /// The commit that gave the path this blob has this committer time.
    Committed(i64),
    /// HEAD holds the blob at the path, and the walk ended before the
    /// commit that gave it.
    Unread,
    /// HEAD holds no such blob at the path, or no HEAD commit was read.
    NotHeld,
}

/// The committer times the reader found, for the [`MEMO_ROOTS`] roots asked
/// about last: for a path of a root, the blob and the time of the commit
/// that gave the path that blob. An entry answers for the same bytes at the
/// same path, so a pass at unchanged files asks the reader nothing. A
/// history rewritten to give the same bytes another commit keeps the first
/// answer while the entry is held.
#[derive(Default)]
struct Memo {
    roots: HashMap<PathBuf, HashMap<String, (gix::ObjectId, i64)>>,
    /// The held roots, the one asked about longest ago first.
    order: VecDeque<PathBuf>,
}

impl Memo {
    fn held(&self, root: &Path, path: &str, blob: gix::ObjectId) -> Option<i64> {
        let (held, at_ms) = self.roots.get(root)?.get(path)?;
        (*held == blob).then_some(*at_ms)
    }

    /// Makes the held root `root` the root asked about last.
    fn touch(&mut self, root: &Path) {
        if let Some(at) = self.order.iter().position(|held| held == root) {
            let root = self.order.remove(at).expect("a position of the order");
            self.order.push_back(root);
        }
    }

    /// Holds that the commit that gave `path` the bytes `blob` has the
    /// time `at_ms`, and makes `root` the root asked about last. A root at
    /// its file bound starts anew.
    fn hold(&mut self, root: &Path, path: &str, blob: gix::ObjectId, at_ms: i64) {
        self.order.retain(|held| held != root);
        if self.order.len() == MEMO_ROOTS
            && let Some(oldest) = self.order.pop_front()
        {
            self.roots.remove(&oldest);
        }
        self.order.push_back(root.to_owned());
        let files = self.roots.entry(root.to_owned()).or_default();
        if files.len() == MEMO_FILES && !files.contains_key(path) {
            files.clear();
        }
        files.insert(path.to_owned(), (blob, at_ms));
    }
}

/// One pass's question to the reader: the files of `root` whose bytes on
/// disk hash to these blobs.
struct Job {
    root: PathBuf,
    files: Vec<(String, gix::ObjectId)>,
    answer: SyncSender<Vec<Found>>,
}

/// The reader thread's queue and the memo of its answers.
pub(crate) struct History {
    jobs: SyncSender<Job>,
    memo: Arc<Mutex<Memo>>,
}

impl History {
    /// Starts a reader thread. When the thread cannot start, every question
    /// goes unanswered.
    fn start() -> Self {
        let (jobs, queued) = sync_channel::<Job>(QUEUE);
        let memo = Arc::new(Mutex::new(Memo::default()));
        let answers = Arc::clone(&memo);
        let started = std::thread::Builder::new()
            .name("named-file-history".to_owned())
            .spawn(move || read_jobs(&queued, &answers));
        if let Err(error) = started {
            eprintln!("daemon: named file history reader did not start: {error}");
        }
        Self { jobs, memo }
    }

    /// The daemon's reader, started when a pass first asks it.
    pub(crate) fn shared() -> &'static Self {
        static SHARED: OnceLock<History> = OnceLock::new();
        SHARED.get_or_init(Self::start)
    }

    fn memo(&self) -> std::sync::MutexGuard<'_, Memo> {
        self.memo.lock().expect("named file history memo mutex")
    }

    /// What HEAD's history says of each of `files`, from the memo or from
    /// the reader within [`WAIT`]. `None` for a file the reader gave no
    /// answer for in that time, and for every file when the queue is full.
    fn ask(&self, root: &Path, files: &[(&str, gix::ObjectId)]) -> Vec<Option<Found>> {
        let mut found: Vec<Option<Found>> = {
            let mut memo = self.memo();
            let found: Vec<Option<Found>> = files
                .iter()
                .map(|(path, blob)| memo.held(root, path, *blob).map(Found::Committed))
                .collect();
            if found.iter().any(Option::is_some) {
                memo.touch(root);
            }
            found
        };
        let asked: Vec<usize> = (0..files.len())
            .filter(|index| found[*index].is_none())
            .collect();
        if asked.is_empty() {
            return found;
        }
        let (answer, answered) = sync_channel(1);
        let job = Job {
            root: root.to_owned(),
            files: asked
                .iter()
                .map(|index| (files[*index].0.to_owned(), files[*index].1))
                .collect(),
            answer,
        };
        match self.jobs.try_send(job) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => return found,
        }
        if let Ok(answers) = answered.recv_timeout(WAIT) {
            for (index, answer) in asked.into_iter().zip(answers) {
                found[index] = Some(answer);
            }
        }
        found
    }
}

/// The reader thread: answers each queued job and holds the committer
/// times it found. A pass that stopped waiting leaves its answer unread.
fn read_jobs(queued: &Receiver<Job>, memo: &Mutex<Memo>) {
    for job in queued {
        let found = read_history(&job.root, &job.files, Instant::now() + JOB_BUDGET);
        {
            let mut memo = memo.lock().expect("named file history memo mutex");
            for ((path, blob), found) in job.files.iter().zip(&found) {
                if let Found::Committed(at_ms) = found {
                    memo.hold(&job.root, path, *blob, *at_ms);
                }
            }
        }
        let _ = job.answer.try_send(found);
    }
}

fn millis(seconds: i64) -> i64 {
    seconds.saturating_mul(1_000)
}

/// A commit as the walk reads it.
struct Commit {
    tree: gix::ObjectId,
    parents: Vec<gix::ObjectId>,
    committed_ms: i64,
}

/// Reads commits and trees of one repository until `deadline`.
struct Reader<'repo> {
    objects: Objects<'repo>,
    hash: gix::hash::Kind,
    deadline: Instant,
}

impl Reader<'_> {
    fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    /// The commit `id`: its whole object decodes, is a commit, and hashes
    /// to `id`.
    fn commit(&mut self, id: gix::ObjectId) -> Result<Commit, ()> {
        if self.expired() {
            return Err(());
        }
        let bytes = self
            .objects
            .read(id, ObjectClass::Commit, &mut 0)
            .map_err(drop)?;
        let commit = gix::objs::CommitRef::from_bytes(bytes, self.hash).map_err(drop)?;
        Ok(Commit {
            tree: commit.tree(),
            parents: commit.parents().collect(),
            committed_ms: millis(commit.time().map_err(drop)?.seconds),
        })
    }

    /// The blob the tree `id` holds at `path`: a regular or executable
    /// file. `Ok(None)` for a path the tree holds nothing or another kind
    /// of entry at, and `Err` for a tree that cannot be read whole.
    fn blob_at(&mut self, mut id: gix::ObjectId, path: &str) -> Result<Option<gix::ObjectId>, ()> {
        use gix::objs::tree::EntryKind;
        let mut components = path.split('/').peekable();
        while let Some(component) = components.next() {
            if self.expired() {
                return Err(());
            }
            let tree = self
                .objects
                .read(id, ObjectClass::Tree, &mut 0)
                .map_err(drop)?;
            let mut entry = None;
            for decoded in gix::objs::TreeRefIter::from_bytes(tree, self.hash) {
                let decoded = decoded.map_err(drop)?;
                if entry.is_none() && decoded.filename == component.as_bytes() {
                    entry = Some((decoded.mode.kind(), decoded.oid.to_owned()));
                }
            }
            let last = components.peek().is_none();
            match (entry, last) {
                (Some((EntryKind::Tree, next)), false) => id = next,
                (Some((EntryKind::Blob | EntryKind::BlobExecutable, blob)), true) => {
                    return Ok(Some(blob));
                }
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    /// What the history of `head` says of `path`, whose bytes on disk hash
    /// to `blob`. The walk starts at `head` and moves to the first parent
    /// that holds `blob` at the path. It ends at a commit none of whose
    /// parents holds it, which is the commit that gave the path the blob.
    /// It ends unread at the first parent or tree it cannot read, as a
    /// shallow clone has, after [`MAX_PARENTS_READ`] parents, and at the
    /// deadline.
    fn walk(&mut self, head: gix::ObjectId, path: &str, blob: gix::ObjectId) -> Found {
        let Ok(mut commit) = self.commit(head) else {
            return Found::NotHeld;
        };
        if self.blob_at(commit.tree, path) != Ok(Some(blob)) {
            return Found::NotHeld;
        }
        let mut read = 0;
        loop {
            let mut same = None;
            for parent in &commit.parents {
                if read == MAX_PARENTS_READ {
                    return Found::Unread;
                }
                read += 1;
                let Ok(parent) = self.commit(*parent) else {
                    return Found::Unread;
                };
                match self.blob_at(parent.tree, path) {
                    Ok(Some(held)) if held == blob => {
                        same = Some(parent);
                        break;
                    }
                    Ok(_) => {}
                    Err(()) => return Found::Unread,
                }
            }
            match same {
                Some(parent) => commit = parent,
                None => return Found::Committed(commit.committed_ms),
            }
        }
    }
}

/// What HEAD's history says of each of `files` of the repository whose work
/// tree is `root`, read until `deadline`. Every file is [`Found::NotHeld`]
/// when `root` opens as no work tree of a SHA-1 repository or HEAD names no
/// commit [`observe_head`] reads.
fn read_history(root: &Path, files: &[(String, gix::ObjectId)], deadline: Instant) -> Vec<Found> {
    let not_held = || vec![Found::NotHeld; files.len()];
    let FrozenHead::Commit(head) = observe_head(root) else {
        return not_held();
    };
    let slots = gix::odb::store::init::Slots::Given(INDEX_SLOTS);
    let Ok(repo) = crate::git_sources::open_with_slots(root, ALLOCATION_LIMIT, slots) else {
        return not_held();
    };
    let hash = repo.object_hash();
    let Some(head) = crate::git_sources::parse_oid(&head, hash) else {
        return not_held();
    };
    if repo.workdir().is_none() || hash != gix::hash::Kind::Sha1 {
        return not_held();
    }
    let mut reader = Reader {
        objects: Objects::new(root, &repo),
        hash,
        deadline,
    };
    files
        .iter()
        .map(|(path, blob)| reader.walk(head, path, *blob))
        .collect()
}

/// One reading of a file on disk: its modification time and, for a file of
/// at most [`MAX_HASHED_BYTES`], the Git blob id of its bytes.
struct Sample {
    modified_ms: i64,
    blob: Option<gix::ObjectId>,
}

/// Reads the regular file at `relative` beneath `root` through one
/// descriptor: its status, its bytes, and its status again. A reading whose
/// two statuses differ, as a write during the read leaves, is taken once
/// more. `None` for a path that names no regular file, a file that cannot
/// be read, and a file whose second reading changed as well.
fn sample(root: BorrowedFd<'_>, relative: &str) -> Option<Sample> {
    use rustix::fs::{FileType, OFlags};
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let identity = |stat: &rustix::fs::Stat| {
        (
            stat.st_ino,
            stat.st_size,
            stat.st_mtime,
            stat.st_mtime_nsec,
            stat.st_ctime,
            stat.st_ctime_nsec,
        )
    };
    crate::confined_path::check(relative).ok()?;
    for _ in 0..2 {
        let fd = crate::confined_path::open_beneath(root, relative, flags).ok()?;
        let before = rustix::fs::fstat(&fd).ok()?;
        if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile {
            return None;
        }
        let modified_ms = before
            .st_mtime
            .saturating_mul(1_000)
            .saturating_add(i64::try_from(before.st_mtime_nsec / 1_000_000).unwrap_or(0));
        let size = u64::try_from(before.st_size).ok()?;
        if size > MAX_HASHED_BYTES {
            return Some(Sample {
                modified_ms,
                blob: None,
            });
        }
        // One byte past the size shows a file that grew since its status.
        let mut bytes = Vec::with_capacity(size as usize + 1);
        let mut file = std::fs::File::from(fd);
        (&mut file).take(size + 1).read_to_end(&mut bytes).ok()?;
        let after = rustix::fs::fstat(&file).ok()?;
        if identity(&before) == identity(&after) && bytes.len() as u64 == size {
            let blob =
                gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, &bytes).ok();
            return Some(Sample { modified_ms, blob });
        }
    }
    None
}

/// When each of `paths`, relative to `root`, last changed. `root_fd` is
/// `root`, opened. `None` for a path [`sample`] reads no file at. A file
/// whose bytes HEAD's history gives a commit for shows that commit's time;
/// one whose history the reader did not follow to that commit shows
/// [`LastChange::HistoryUnread`]; any other shows the modification time its
/// sample read. No path opens no repository.
pub(crate) fn last_changes(
    history: &History,
    root: &Path,
    root_fd: BorrowedFd<'_>,
    paths: &[&str],
) -> Vec<Option<LastChange>> {
    let samples: Vec<Option<Sample>> = paths.iter().map(|path| sample(root_fd, path)).collect();
    let hashed: Vec<(usize, &str, gix::ObjectId)> = samples
        .iter()
        .enumerate()
        .filter_map(|(index, sample)| Some((index, paths[index], sample.as_ref()?.blob?)))
        .collect();
    let mut found: Vec<Option<Found>> = vec![None; paths.len()];
    if !hashed.is_empty() {
        let files: Vec<(&str, gix::ObjectId)> = hashed
            .iter()
            .map(|(_, path, blob)| (*path, *blob))
            .collect();
        for ((index, _, _), answer) in hashed.iter().zip(history.ask(root, &files)) {
            found[*index] = answer;
        }
    }
    samples
        .into_iter()
        .zip(found)
        .map(|(sample, found)| {
            let sample = sample?;
            Some(match found {
                Some(Found::Committed(at_ms)) => LastChange::Committed(at_ms),
                Some(Found::Unread) => LastChange::HistoryUnread,
                Some(Found::NotHeld) | None => LastChange::OnDisk(sample.modified_ms),
            })
        })
        .collect()
}

/// Repositories for the tests of the last change: commits with chosen
/// committer times, written through gix's object APIs.
#[cfg(test)]
pub(crate) mod fixtures {
    use std::path::Path;

    fn signature(seconds: i64) -> gix::actor::Signature {
        gix::actor::Signature {
            name: "fixture".into(),
            email: "fixture@example.com".into(),
            time: gix::date::Time::new(seconds, 0),
        }
    }

    /// Commits `files` as the whole tree, with the committer time
    /// `seconds` and the author time 1, on `parents`, and returns the
    /// commit. A file is a path of at most two components and its text.
    pub(crate) fn commit(
        repo: &gix::Repository,
        seconds: i64,
        parents: &[gix::ObjectId],
        files: &[(&str, &str)],
    ) -> gix::ObjectId {
        use gix::objs::tree::{Entry, EntryKind};
        let blob = |text: &str| repo.write_blob(text).unwrap().detach();
        let mut top: Vec<Entry> = Vec::new();
        let mut dirs: std::collections::BTreeMap<&str, Vec<Entry>> = Default::default();
        for (path, text) in files {
            let entry = |name: &str| Entry {
                mode: EntryKind::Blob.into(),
                filename: name.into(),
                oid: blob(text),
            };
            match path.split_once('/') {
                Some((dir, name)) => dirs.entry(dir).or_default().push(entry(name)),
                None => top.push(entry(path)),
            }
        }
        for (dir, mut entries) in dirs {
            entries.sort();
            let tree = repo
                .write_object(gix::objs::Tree { entries })
                .unwrap()
                .detach();
            top.push(Entry {
                mode: EntryKind::Tree.into(),
                filename: dir.into(),
                oid: tree,
            });
        }
        top.sort();
        let tree = repo
            .write_object(gix::objs::Tree { entries: top })
            .unwrap()
            .detach();
        let commit = gix::objs::Commit {
            tree,
            parents: parents.iter().copied().collect(),
            author: signature(1),
            committer: signature(seconds),
            encoding: None,
            message: "fixture".into(),
            extra_headers: Vec::new(),
        };
        repo.write_object(commit).unwrap().detach()
    }

    /// Makes `commit` the commit HEAD names and writes `files` to the work
    /// tree.
    pub(crate) fn check_out(root: &Path, commit: gix::ObjectId, files: &[(&str, &str)]) {
        std::fs::write(root.join(".git/HEAD"), format!("{commit}\n")).unwrap();
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    /// Makes `root` the work tree of a new repository.
    pub(crate) fn repository_at(root: &Path) -> gix::Repository {
        gix::init(root).unwrap();
        gix::open_opts(root, gix::open::Options::isolated()).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{check_out, commit, repository_at};
    use super::*;
    use std::os::fd::AsFd as _;

    fn repository() -> (tempfile::TempDir, gix::Repository) {
        let dir = tempfile::tempdir().unwrap();
        let repo = repository_at(dir.path());
        (dir, repo)
    }

    /// The last change of each of `paths`, asked of a reader of this test's
    /// own.
    fn changes(root: &Path, paths: &[&str]) -> Vec<Option<LastChange>> {
        let fd = std::fs::File::open(root).unwrap();
        last_changes(&History::start(), root, fd.as_fd(), paths)
    }

    /// The modification time the file system holds for `path`.
    fn on_disk(root: &Path, path: &str) -> Option<LastChange> {
        let modified = std::fs::metadata(root.join(path))
            .unwrap()
            .modified()
            .unwrap();
        let since = modified.duration_since(std::time::UNIX_EPOCH).unwrap();
        Some(LastChange::OnDisk(since.as_millis() as i64))
    }

    fn committed(seconds: i64) -> Option<LastChange> {
        Some(LastChange::Committed(seconds * 1_000))
    }

    const DAY: i64 = 86_400;

    /// A file last changed with the newest commit after which HEAD's blob
    /// stayed, whatever later commits changed beside it and whatever the
    /// commit's author time is. A file HEAD does not hold and a file whose
    /// bytes differ from HEAD's blob show the modification time of the
    /// file read, and a directory and an absent path show nothing.
    #[test]
    fn a_committed_file_changed_with_the_commit_that_gave_it_its_bytes() {
        let (dir, repo) = repository();
        let root = dir.path();
        let first = commit(
            &repo,
            DAY,
            &[],
            &[("docs/style.md", "one\n"), ("README.md", "r\n")],
        );
        let second = commit(
            &repo,
            2 * DAY,
            &[first],
            &[("docs/style.md", "two\n"), ("README.md", "r\n")],
        );
        let third = commit(
            &repo,
            3 * DAY,
            &[second],
            &[
                ("docs/style.md", "two\n"),
                ("README.md", "r\n"),
                ("tools/check.sh", "exit 0\n"),
            ],
        );
        check_out(
            root,
            third,
            &[
                ("docs/style.md", "two\n"),
                ("README.md", "edited\n"),
                ("tools/check.sh", "exit 0\n"),
                ("notes.txt", "untracked\n"),
            ],
        );
        assert_eq!(
            changes(
                root,
                &[
                    "docs/style.md",
                    "README.md",
                    "tools/check.sh",
                    "notes.txt",
                    "docs",
                    "absent.md"
                ]
            ),
            [
                committed(2 * DAY),
                on_disk(root, "README.md"),
                committed(3 * DAY),
                on_disk(root, "notes.txt"),
                None,
                None,
            ]
        );
        let fourth = commit(
            &repo,
            4 * DAY,
            &[third],
            &[("docs/style.md", "four\n"), ("README.md", "r\n")],
        );
        check_out(root, fourth, &[("docs/style.md", "four\n")]);
        assert_eq!(
            changes(root, &["docs/style.md", "tools/check.sh"]),
            [committed(4 * DAY), on_disk(root, "tools/check.sh")]
        );
    }

    /// A merge that takes a file from one parent leaves the file's change
    /// with that parent's history; a merge that writes bytes neither parent
    /// holds is the change.
    #[test]
    fn a_merge_follows_the_parent_that_holds_the_same_bytes() {
        let (dir, repo) = repository();
        let root = dir.path();
        let base = commit(&repo, DAY, &[], &[("a.md", "base\n"), ("b.md", "base\n")]);
        let left = commit(
            &repo,
            2 * DAY,
            &[base],
            &[("a.md", "left\n"), ("b.md", "base\n")],
        );
        let right = commit(
            &repo,
            3 * DAY,
            &[base],
            &[("a.md", "base\n"), ("b.md", "right\n")],
        );
        let merged = [("a.md", "left\n"), ("b.md", "merged\n")];
        let merge = commit(&repo, 4 * DAY, &[right, left], &merged);
        check_out(root, merge, &merged);
        assert_eq!(
            changes(root, &["a.md", "b.md"]),
            [committed(2 * DAY), committed(4 * DAY)]
        );
    }

    /// A walk reads 256 parents: a file unchanged over more commits than
    /// that gives no time, and one commit fewer gives the first commit's.
    #[test]
    fn a_walk_past_256_parents_gives_no_time() {
        let (dir, repo) = repository();
        let root = dir.path();
        let files = [("a.md", "same\n")];
        let mut tip = commit(&repo, DAY, &[], &files);
        for index in 0..=256 {
            tip = commit(&repo, DAY + 1 + index, &[tip], &files);
        }
        check_out(root, tip, &files);
        // The tip is the 258th commit: the walk reads its 256 ancestors at
        // DAY + 256 down to DAY + 1 and stops with the first commit unread.
        assert_eq!(changes(root, &["a.md"]), [Some(LastChange::HistoryUnread)]);
        let one_fewer = repo.find_commit(tip).unwrap().parent_ids().next().unwrap();
        check_out(root, one_fewer.detach(), &files);
        assert_eq!(changes(root, &["a.md"]), [committed(DAY)]);
    }

    /// A walk ends unread at the first parent it cannot read, also when a
    /// later parent holds the same bytes, and at a parent whose tree it
    /// cannot read. A parent that introduced the bytes with a later
    /// committer time than its child gives its own time once it is read.
    #[test]
    fn a_walk_ends_unread_at_the_first_parent_or_tree_it_cannot_read() {
        let files = [("docs/a.md", "same\n")];
        let absent = gix::ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();

        let (dir, repo) = repository();
        let shallow = commit(&repo, 5 * DAY, &[absent], &files);
        check_out(dir.path(), shallow, &files);
        assert_eq!(
            changes(dir.path(), &["docs/a.md"]),
            [Some(LastChange::HistoryUnread)]
        );

        // The first parent is absent and the second, readable, holds the
        // bytes with an earlier time.
        let (dir, repo) = repository();
        let earlier = commit(&repo, DAY, &[], &files);
        let merge = commit(&repo, 5 * DAY, &[absent, earlier], &files);
        check_out(dir.path(), merge, &files);
        assert_eq!(
            changes(dir.path(), &["docs/a.md"]),
            [Some(LastChange::HistoryUnread)]
        );

        // The parent commit is present and the subtree of the path is not.
        let (dir, repo) = repository();
        let tree = repo
            .write_object(gix::objs::Tree {
                entries: vec![gix::objs::tree::Entry {
                    mode: gix::objs::tree::EntryKind::Tree.into(),
                    filename: "docs".into(),
                    oid: absent,
                }],
            })
            .unwrap()
            .detach();
        let signature = gix::actor::Signature {
            name: "fixture".into(),
            email: "fixture@example.com".into(),
            time: gix::date::Time::new(DAY, 0),
        };
        let parent = repo
            .write_object(gix::objs::Commit {
                tree,
                parents: Default::default(),
                author: signature.clone(),
                committer: signature,
                encoding: None,
                message: "fixture".into(),
                extra_headers: Vec::new(),
            })
            .unwrap()
            .detach();
        let child = commit(&repo, 5 * DAY, &[parent], &files);
        check_out(dir.path(), child, &files);
        assert_eq!(
            changes(dir.path(), &["docs/a.md"]),
            [Some(LastChange::HistoryUnread)]
        );

        // A parent dated after its child gave the file its bytes.
        let (dir, repo) = repository();
        let later = commit(&repo, 9 * DAY, &[], &files);
        let child = commit(&repo, 5 * DAY, &[later], &files);
        check_out(dir.path(), child, &files);
        assert_eq!(changes(dir.path(), &["docs/a.md"]), [committed(9 * DAY)]);
    }

    /// A directory that opens as no repository, a repository with no
    /// commit, a HEAD that names a tag object, and a file past the hashed
    /// size show the modification time.
    #[test]
    fn a_file_without_readable_history_shows_its_modification_time() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "text\n").unwrap();
        assert_eq!(
            changes(dir.path(), &["a.md"]),
            [on_disk(dir.path(), "a.md")]
        );

        let (dir, repo) = repository();
        let root = dir.path();
        std::fs::write(root.join("a.md"), "text\n").unwrap();
        assert_eq!(changes(root, &["a.md"]), [on_disk(root, "a.md")]);

        let at_bound = "x".repeat(256 * 1024);
        let past_bound = format!("{at_bound}x");
        let files = [("at.txt", at_bound.as_str()), ("past.txt", &past_bound)];
        let only = commit(&repo, DAY, &[], &files);
        check_out(root, only, &files);
        assert_eq!(
            changes(root, &["at.txt", "past.txt"]),
            [committed(DAY), on_disk(root, "past.txt")]
        );

        // A detached HEAD at a tag object whose body is no tag.
        let tag = {
            use gix::objs::Write as _;
            repo.write_buf(gix::object::Kind::Tag, b"no tag header\n")
                .unwrap()
        };
        std::fs::write(root.join(".git/HEAD"), format!("{tag}\n")).unwrap();
        assert_eq!(changes(root, &["at.txt"]), [on_disk(root, "at.txt")]);
    }

    /// A HEAD commit, and a parent commit, stored in a pack as a delta that
    /// names itself as its base: the reader ends the read, so the file of
    /// the first shows its modification time and the file of the second no
    /// time, and a commit packed whole gives its time.
    #[test]
    fn a_delta_chain_that_revisits_an_entry_ends_the_read() {
        use crate::memory_capture::head_fixtures::{Delta, pack_objects};
        let files = [("a.md", "same\n")];
        let pack = |root: &Path, id: gix::ObjectId, delta: Option<Delta>| {
            pack_objects(root, &[&id.to_string()], delta);
        };

        let (dir, repo) = repository();
        let head = commit(&repo, DAY, &[], &files);
        check_out(dir.path(), head, &files);
        pack(dir.path(), head, None);
        assert_eq!(changes(dir.path(), &["a.md"]), [committed(DAY)]);

        let (dir, repo) = repository();
        let head = commit(&repo, DAY, &[], &files);
        check_out(dir.path(), head, &files);
        pack(dir.path(), head, Some(Delta::SelfId));
        let started = Instant::now();
        assert_eq!(
            changes(dir.path(), &["a.md"]),
            [on_disk(dir.path(), "a.md")]
        );

        let (dir, repo) = repository();
        let parent = commit(&repo, DAY, &[], &files);
        let head = commit(&repo, 2 * DAY, &[parent], &files);
        check_out(dir.path(), head, &files);
        pack(dir.path(), parent, Some(Delta::SelfId));
        assert_eq!(
            changes(dir.path(), &["a.md"]),
            [Some(LastChange::HistoryUnread)]
        );
        assert!(started.elapsed() < WAIT, "{:?}", started.elapsed());
    }

    /// A repository whose HEAD is a pipe nobody writes blocks the reader and
    /// no pass: the pass waits its bound and shows the modification time,
    /// and once the queue holds its bound of passes a pass waits for
    /// nothing.
    #[test]
    fn a_repository_that_blocks_the_reader_costs_a_pass_its_wait() {
        let (dir, repo) = repository();
        let root = dir.path();
        let files = [("a.md", "same\n")];
        let head = commit(&repo, DAY, &[], &files);
        check_out(root, head, &files);
        std::fs::remove_file(root.join(".git/HEAD")).unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            root.join(".git/HEAD"),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        let history = History::start();
        let fd = std::fs::File::open(root).unwrap();
        let ask = || last_changes(&history, root, fd.as_fd(), &["a.md"]);
        let started = Instant::now();
        assert_eq!(ask(), [on_disk(root, "a.md")]);
        let waited = started.elapsed();
        assert!(waited >= WAIT && waited < WAIT * 3, "{waited:?}");
        // The reader holds the first job; four more fill the queue.
        for _ in 0..QUEUE {
            assert_eq!(ask(), [on_disk(root, "a.md")]);
        }
        let started = Instant::now();
        assert_eq!(ask(), [on_disk(root, "a.md")]);
        assert!(started.elapsed() < WAIT / 2, "{:?}", started.elapsed());
        // No path asks nothing and waits for nothing.
        let started = Instant::now();
        assert!(last_changes(&history, root, fd.as_fd(), &[]).is_empty());
        assert!(started.elapsed() < WAIT / 2);
    }

    /// A pass at unchanged bytes is answered from the memo: the reader's
    /// answer for a path and its blob stands while the file hashes to that
    /// blob, also once the repository is gone, and other bytes ask again.
    #[test]
    fn unchanged_bytes_are_answered_from_the_memo() {
        let (dir, repo) = repository();
        let root = dir.path();
        let files = [("a.md", "same\n")];
        let head = commit(&repo, DAY, &[], &files);
        check_out(root, head, &files);
        let history = History::start();
        let fd = std::fs::File::open(root).unwrap();
        let ask = || last_changes(&history, root, fd.as_fd(), &["a.md"]);
        assert_eq!(ask(), [committed(DAY)]);
        std::fs::remove_dir_all(root.join(".git")).unwrap();
        assert_eq!(ask(), [committed(DAY)]);
        std::fs::write(root.join("a.md"), "other\n").unwrap();
        assert_eq!(ask(), [on_disk(root, "a.md")]);
    }

    /// The memo holds 64 roots and 64 paths of each, and answers for the
    /// blob it holds at a path.
    #[test]
    fn the_memo_holds_64_roots_and_64_paths_of_each() {
        let id = |byte: u8| gix::ObjectId::from_bytes_or_panic(&[byte; 20]);
        let mut memo = Memo::default();
        for index in 0..64 {
            memo.hold(Path::new(&format!("/r{index}")), "a.md", id(1), 5);
        }
        memo.hold(Path::new("/r0"), "b.md", id(2), 6);
        // A root a pass read an answer of is asked about last as well.
        memo.touch(Path::new("/r1"));
        memo.hold(Path::new("/r64"), "a.md", id(1), 5);
        assert_eq!((memo.roots.len(), memo.order.len()), (64, 64));
        assert_eq!(memo.held(Path::new("/r0"), "a.md", id(1)), Some(5));
        assert_eq!(memo.held(Path::new("/r0"), "b.md", id(2)), Some(6));
        assert_eq!(memo.held(Path::new("/r0"), "a.md", id(2)), None);
        assert_eq!(memo.held(Path::new("/r1"), "a.md", id(1)), Some(5));
        assert_eq!(memo.held(Path::new("/r2"), "a.md", id(1)), None);
        // A path's new blob takes the place of its old one.
        memo.hold(Path::new("/r0"), "a.md", id(3), 7);
        assert_eq!(memo.held(Path::new("/r0"), "a.md", id(1)), None);
        assert_eq!(memo.held(Path::new("/r0"), "a.md", id(3)), Some(7));
        // The 65th path of a root starts the root anew.
        for index in 1..64 {
            memo.hold(Path::new("/r3"), &format!("f{index}.md"), id(1), 5);
        }
        assert_eq!(memo.roots[Path::new("/r3")].len(), 64);
        memo.hold(Path::new("/r3"), "f1.md", id(4), 8);
        assert_eq!(memo.roots[Path::new("/r3")].len(), 64);
        memo.hold(Path::new("/r3"), "one-more.md", id(1), 5);
        assert_eq!(memo.roots[Path::new("/r3")].len(), 1);
    }
}

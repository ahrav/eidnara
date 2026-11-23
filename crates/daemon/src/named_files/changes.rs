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
//! bytes differ from that blob, a file whose bytes the pass did not read
//! whole, and a file of a project whose root is the work tree of no
//! SHA-1 repository or whose HEAD names no commit the reader reads.
//!
//! A pass reads the file and no repository. One thread, the reader, reads
//! the repository for every pass, and a pass waits at most [`WAIT`] for its
//! answer, so a repository that blocks or spins a read costs a pass that
//! wait and the line its commit time. The reader takes HEAD through
//! [`crate::memory_capture::observe_head`], which capture calls for every
//! session it captures, and each commit and tree through the attestation
//! verifier's object reader, which returns an object whose bytes hash to
//! its id. Those two readers read the repository's own files, its HEAD,
//! references, configuration, and object and pack files, with the bounds
//! and the file mappings they have for capture and for verification, under
//! the cooperative-file model `vector_reader` states: a pack or object
//! file truncated while it is mapped raises `SIGBUS` in the daemon, as it
//! does during capture's read. This module bounds the parents a walk
//! reads, the time the reader works for a pass, and the answers it keeps.

use std::collections::{HashMap, VecDeque};
use std::io::Read as _;
use std::os::fd::BorrowedFd;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use crate::attestation_verifier::{ALLOCATION_LIMIT, INDEX_SLOTS, ObjectClass, Objects};
use crate::memory_capture::{FrozenHead, observe_head};

/// When a file last changed, as one delivery reads it. A time is in
/// milliseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LastChange {
    /// The committer time of the commit that gave the file its bytes.
    Committed(i64),
    /// The reader read HEAD's commit, and the history it read does not
    /// show which commit gave the file its bytes: the walk would read a
    /// parent past its bound, or met a commit or a tree it could not read.
    HistoryUnread,
    /// The modification time of the file on disk.
    OnDisk(i64),
}

/// Parents one file's walk reads.
const MAX_PARENTS_READ: usize = 256;
/// The largest file whose bytes are hashed against the blob HEAD holds.
const MAX_HASHED_BYTES: u64 = 256 * 1024;
/// How long a pass waits for the reader's answer, and so how long the
/// reader works for the pass: it checks the time before each object it
/// reads and takes up no job whose pass has stopped waiting.
const WAIT: Duration = Duration::from_secs(1);
/// Jobs that wait in the queue beside the one the reader works on. A pass
/// that finds the queue full asks nothing.
const QUEUE: usize = 4;
/// Project roots whose answers the reader keeps.
const KEPT_ROOTS: usize = 64;
/// Paths of one root whose answers the reader keeps.
const KEPT_FILES: usize = 64;

/// The resident bytes the kept answers and the queue hold at their bounds:
/// for each of [`KEPT_ROOTS`] roots its path and, for each of
/// [`KEPT_FILES`] files, a path of at most 256 bytes, a blob id, and an
/// answer; for each of [`QUEUE`] waiting jobs and the one the reader works
/// on, a root path and eight files.
pub(crate) const RETAINED_BYTES_BOUND: u64 =
    (KEPT_ROOTS as u64) * (8 * 1024 + (KEPT_FILES as u64) * 512) + (QUEUE as u64 + 1) * 16 * 1024;

/// What HEAD's history says of one file: the path and the blob a pass read
/// on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    /// The commit that gave the path this blob has this committer time.
    Committed(i64),
    /// HEAD's commit was read, and the read ended before it showed the
    /// commit that gave the path the blob.
    Unread,
    /// HEAD holds no such blob at the path, or no HEAD commit was read.
    NotHeld,
}

/// How one file's read ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Walked {
    /// The objects read show this answer, and every later read at the same
    /// HEAD shows it too.
    Settled(Found),
    /// An object the read needs could not be read. A repository that gains
    /// the object, as a shallow clone does when it fetches more history,
    /// answers otherwise at the same HEAD.
    Unreadable(Found),
    /// The pass's wait ended first.
    Stopped,
}

impl Walked {
    fn answer(self) -> Option<Found> {
        match self {
            Self::Settled(found) | Self::Unreadable(found) => Some(found),
            Self::Stopped => None,
        }
    }
}

/// A directory as the file system identifies it: its device and inode.
type Directory = (u64, u64);

/// The settled answers of one root: the directory and the HEAD commit they
/// were read at and, for a path, the blob and the answer.
struct Kept {
    directory: Directory,
    head: gix::ObjectId,
    files: HashMap<String, (gix::ObjectId, Found)>,
}

/// The settled answers the reader keeps, for the [`KEPT_ROOTS`] roots
/// asked about last. An answer stands for its path and blob while the root
/// is the same directory at the same HEAD commit, so a pass at an
/// unchanged HEAD costs the reader the read of HEAD and no walk. The
/// reader thread alone reads and writes them.
#[derive(Default)]
struct Answers {
    roots: HashMap<PathBuf, Kept>,
    /// The kept roots, the one asked about longest ago first.
    order: VecDeque<PathBuf>,
}

impl Answers {
    /// The answers kept for `root`, made the root asked about last: those
    /// read in `directory` at `head`, or none after another directory or
    /// HEAD. A 65th root takes the place of the one asked about longest
    /// ago.
    fn at(
        &mut self,
        root: &Path,
        directory: Directory,
        head: gix::ObjectId,
    ) -> &mut HashMap<String, (gix::ObjectId, Found)> {
        self.order.retain(|kept| kept != root);
        if self.order.len() == KEPT_ROOTS
            && let Some(oldest) = self.order.pop_front()
        {
            self.roots.remove(&oldest);
        }
        self.order.push_back(root.to_owned());
        let kept = self.roots.entry(root.to_owned()).or_insert_with(|| Kept {
            directory,
            head,
            files: HashMap::new(),
        });
        if (kept.directory, kept.head) != (directory, head) {
            kept.directory = directory;
            kept.head = head;
            kept.files.clear();
        }
        &mut kept.files
    }
}

/// Keeps `found` for `path` and `blob`. A root at its file bound starts
/// anew.
fn keep(
    files: &mut HashMap<String, (gix::ObjectId, Found)>,
    path: &str,
    blob: gix::ObjectId,
    found: Found,
) {
    if files.len() == KEPT_FILES && !files.contains_key(path) {
        files.clear();
    }
    files.insert(path.to_owned(), (blob, found));
}

/// One pass's question to the reader: the files of `root`, which the pass
/// opened as `directory`, whose bytes on disk hash to these blobs. The
/// pass waits for the answer until `until`.
struct Job {
    root: PathBuf,
    directory: Directory,
    files: Vec<(String, gix::ObjectId)>,
    until: Instant,
    answer: SyncSender<Vec<Option<Found>>>,
}

/// The queue of the reader thread.
pub(crate) struct History {
    jobs: SyncSender<Job>,
}

impl History {
    /// Starts a reader thread. When the thread cannot start, every question
    /// goes unanswered.
    fn start() -> Self {
        let (jobs, queued) = sync_channel::<Job>(QUEUE);
        let started = std::thread::Builder::new()
            .name("named-file-history".to_owned())
            .spawn(move || read_jobs(&queued));
        if let Err(error) = started {
            eprintln!("daemon: named file history reader did not start: {error}");
        }
        Self { jobs }
    }

    /// The daemon's reader, started when a pass first asks it.
    pub(crate) fn shared() -> &'static Self {
        static SHARED: OnceLock<History> = OnceLock::new();
        SHARED.get_or_init(Self::start)
    }

    /// What HEAD's history says of each of `files` of `root`, which the
    /// pass opened as `directory`: the reader's answer within [`WAIT`].
    /// `None` for a file the reader gave no answer for in that time, and
    /// for every file when the queue is full.
    fn ask(
        &self,
        root: &Path,
        directory: Directory,
        files: &[(&str, gix::ObjectId)],
    ) -> Vec<Option<Found>> {
        let (answer, answered) = sync_channel(1);
        let job = Job {
            root: root.to_owned(),
            directory,
            files: files
                .iter()
                .map(|(path, blob)| ((*path).to_owned(), *blob))
                .collect(),
            until: Instant::now() + WAIT,
            answer,
        };
        if self.jobs.try_send(job).is_err() {
            return vec![None; files.len()];
        }
        answered
            .recv_timeout(WAIT)
            .unwrap_or_else(|_| vec![None; files.len()])
    }
}

/// The reader thread: answers each queued job whose pass still waits, and
/// keeps the answers a HEAD settles.
fn read_jobs(queued: &Receiver<Job>) {
    let mut answers = Answers::default();
    for job in queued {
        if Instant::now() < job.until {
            let found = answer(&mut answers, &job);
            let _ = job.answer.try_send(found);
        }
    }
}

/// The answer to `job`: for each file the kept answer of its path and
/// blob at the HEAD commit read now, or what a walk from that commit
/// finds, which is kept when it is settled. Every file is
/// [`Found::NotHeld`] when [`observe_head`] reads no commit at HEAD, and
/// each file asked of the repository is when `root` opens as no SHA-1
/// repository whose work tree is the directory the pass opened.
fn answer(answers: &mut Answers, job: &Job) -> Vec<Option<Found>> {
    let not_held = || vec![Some(Found::NotHeld); job.files.len()];
    let FrozenHead::Commit(head) = observe_head(&job.root) else {
        return not_held();
    };
    let Some(head) = crate::git_sources::parse_oid(&head, gix::hash::Kind::Sha1) else {
        return not_held();
    };
    let kept = answers.at(&job.root, job.directory, head);
    let mut found: Vec<Option<Walked>> = job
        .files
        .iter()
        .map(|(path, blob)| {
            kept.get(path)
                .filter(|(held, _)| held == blob)
                .map(|(_, found)| Walked::Settled(*found))
        })
        .collect();
    if found.iter().any(Option::is_none) {
        let asked: Vec<&(String, gix::ObjectId)> = job
            .files
            .iter()
            .zip(&found)
            .filter_map(|(file, found)| found.is_none().then_some(file))
            .collect();
        let mut walked =
            read_history(&job.root, job.directory, head, &asked, job.until).into_iter();
        for ((path, blob), slot) in job.files.iter().zip(&mut found) {
            if slot.is_none() {
                let walked = walked.next().expect("an answer for each file asked");
                if let Walked::Settled(found) = walked {
                    keep(kept, path, *blob, found);
                }
                *slot = Some(walked);
            }
        }
    }
    found
        .into_iter()
        .map(|walked| walked.and_then(Walked::answer))
        .collect()
}

fn millis(seconds: i64) -> i64 {
    seconds.saturating_mul(1_000)
}

/// A commit as the walk reads it.
#[derive(Clone)]
struct Commit {
    tree: gix::ObjectId,
    parents: Vec<gix::ObjectId>,
    committed_ms: i64,
}

/// Why a read of an object ended without the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// The object could not be read whole as its kind.
    Unreadable,
    /// The pass's wait ended.
    Expired,
}

impl Ended {
    /// How a walk ends here, where an unreadable object leaves `found`.
    fn walked(self, found: Found) -> Walked {
        match self {
            Self::Unreadable => Walked::Unreadable(found),
            Self::Expired => Walked::Stopped,
        }
    }
}

/// Reads commits and trees of one repository until `until`.
struct Reader<'repo> {
    objects: Objects<'repo>,
    hash: gix::hash::Kind,
    until: Instant,
}

impl Reader<'_> {
    fn expired(&self) -> Result<(), Ended> {
        if Instant::now() < self.until {
            Ok(())
        } else {
            Err(Ended::Expired)
        }
    }

    /// The commit `id`: its whole object decodes, is a commit, hashes to
    /// `id`, and states a committer time that parses.
    fn commit(&mut self, id: gix::ObjectId) -> Result<Commit, Ended> {
        self.expired()?;
        let bytes = self
            .objects
            .read(id, ObjectClass::Commit, &mut 0)
            .map_err(|_| Ended::Unreadable)?;
        let commit =
            gix::objs::CommitRef::from_bytes(bytes, self.hash).map_err(|_| Ended::Unreadable)?;
        let committer = commit.committer().map_err(|_| Ended::Unreadable)?;
        Ok(Commit {
            tree: commit.tree(),
            parents: commit.parents().collect(),
            committed_ms: millis(committer.time().map_err(|_| Ended::Unreadable)?.seconds),
        })
    }

    /// The blob the tree `id` holds at `path`: a regular or executable
    /// file. `Ok(None)` for a path the tree holds nothing or another kind
    /// of entry at.
    fn blob_at(
        &mut self,
        mut id: gix::ObjectId,
        path: &str,
    ) -> Result<Option<gix::ObjectId>, Ended> {
        use gix::objs::tree::EntryKind;
        let mut components = path.split('/').peekable();
        while let Some(component) = components.next() {
            self.expired()?;
            let tree = self
                .objects
                .read(id, ObjectClass::Tree, &mut 0)
                .map_err(|_| Ended::Unreadable)?;
            let mut entry = None;
            for decoded in gix::objs::TreeRefIter::from_bytes(tree, self.hash) {
                let decoded = decoded.map_err(|_| Ended::Unreadable)?;
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

    /// What the history of the commit `head` says of `path`, whose bytes
    /// on disk hash to `blob`. The walk starts at `head` and moves to the
    /// first parent that holds `blob` at the path. It ends at a commit
    /// none of whose parents holds it, which is the commit that gave the
    /// path the blob. It ends unread at a tree of `head` or a parent
    /// commit or tree it cannot read, as a shallow clone has, and where it
    /// would read a parent past [`MAX_PARENTS_READ`].
    fn walk(&mut self, head: &Commit, path: &str, blob: gix::ObjectId) -> Walked {
        match self.blob_at(head.tree, path) {
            Ok(Some(held)) if held == blob => {}
            Ok(_) => return Walked::Settled(Found::NotHeld),
            Err(ended) => return ended.walked(Found::Unread),
        }
        let mut commit = head.clone();
        let mut read = 0;
        loop {
            let mut same = None;
            for parent in &commit.parents {
                if read == MAX_PARENTS_READ {
                    return Walked::Settled(Found::Unread);
                }
                read += 1;
                let parent = match self.commit(*parent) {
                    Ok(parent) => parent,
                    Err(ended) => return ended.walked(Found::Unread),
                };
                match self.blob_at(parent.tree, path) {
                    Ok(Some(held)) if held == blob => {
                        same = Some(parent);
                        break;
                    }
                    Ok(_) => {}
                    Err(ended) => return ended.walked(Found::Unread),
                }
            }
            match same {
                Some(parent) => commit = parent,
                None => return Walked::Settled(Found::Committed(commit.committed_ms)),
            }
        }
    }
}

/// What the history of the commit `head` says of each of `files` of the
/// repository at `root`, read until `until`. Every file is
/// [`Found::NotHeld`], unsettled, when `root` opens as no SHA-1 repository
/// whose work tree is `directory`, or when the commit `head` cannot be
/// read.
fn read_history(
    root: &Path,
    directory: Directory,
    head: gix::ObjectId,
    files: &[&(String, gix::ObjectId)],
    until: Instant,
) -> Vec<Walked> {
    let all = |walked: Walked| vec![walked; files.len()];
    let slots = gix::odb::store::init::Slots::Given(INDEX_SLOTS);
    let Ok(repo) = crate::git_sources::open_with_slots(root, ALLOCATION_LIMIT, slots) else {
        return all(Walked::Unreadable(Found::NotHeld));
    };
    let hash = repo.object_hash();
    let work_tree = repo
        .workdir()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|status| (status.dev(), status.ino()));
    if hash != gix::hash::Kind::Sha1 || work_tree != Some(directory) {
        return all(Walked::Unreadable(Found::NotHeld));
    }
    let mut reader = Reader {
        objects: Objects::new(root, &repo),
        hash,
        until,
    };
    let head = match reader.commit(head) {
        Ok(head) => head,
        Err(ended) => return all(ended.walked(Found::NotHeld)),
    };
    files
        .iter()
        .map(|(path, blob)| reader.walk(&head, path, *blob))
        .collect()
}

/// One reading of a file on disk: its modification time and, for a file of
/// at most [`MAX_HASHED_BYTES`] whose bytes were read whole, the Git blob
/// id of those bytes.
struct Sample {
    modified_ms: i64,
    blob: Option<gix::ObjectId>,
}

fn modified_ms(status: &rustix::fs::Stat) -> i64 {
    status
        .st_mtime
        .saturating_mul(1_000)
        .saturating_add(i64::try_from(status.st_mtime_nsec / 1_000_000).unwrap_or(0))
}

fn is_regular(status: &rustix::fs::Stat) -> bool {
    rustix::fs::FileType::from_raw_mode(status.st_mode) == rustix::fs::FileType::RegularFile
}

/// Reads the regular file at `relative` beneath `root`.
fn sample(root: BorrowedFd<'_>, relative: &str) -> Option<Sample> {
    sample_between(root, relative, &mut || {})
}

/// [`sample`], with `between` called after the first status of each
/// reading, where a test changes the file.
///
/// A reading takes the file's status, at most [`MAX_HASHED_BYTES`] and one
/// byte, and the status again through one descriptor, and holds when the
/// two statuses agree and the bytes are as many as the status states. A
/// reading whose statuses differ, as a write during the read leaves, is
/// taken once more. A file past [`MAX_HASHED_BYTES`] gives the time of its
/// one status and no blob. A file whose bytes no reading holds, one that
/// does not open for reading or that changed during both, gives the
/// modification time of one status taken then and no blob. `None` for a
/// path that names no regular file beneath the root through no symbolic
/// link.
fn sample_between(
    root: BorrowedFd<'_>,
    relative: &str,
    between: &mut dyn FnMut(),
) -> Option<Sample> {
    use rustix::fs::OFlags;
    let identity = |status: &rustix::fs::Stat| {
        (
            status.st_ino,
            status.st_nlink,
            status.st_size,
            status.st_mtime,
            status.st_mtime_nsec,
            status.st_ctime,
            status.st_ctime_nsec,
        )
    };
    crate::confined_path::check(relative).ok()?;
    let read = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    for _ in 0..2 {
        let Ok(fd) = crate::confined_path::open_beneath(root, relative, read) else {
            break;
        };
        let Ok(before) = rustix::fs::fstat(&fd) else {
            break;
        };
        if !is_regular(&before) {
            return None;
        }
        let Ok(size) = u64::try_from(before.st_size) else {
            break;
        };
        if size > MAX_HASHED_BYTES {
            return Some(Sample {
                modified_ms: modified_ms(&before),
                blob: None,
            });
        }
        between();
        // One byte past the size shows a file that grew since its status.
        let mut bytes = Vec::with_capacity(size as usize + 1);
        let mut file = std::fs::File::from(fd);
        if (&mut file).take(size + 1).read_to_end(&mut bytes).is_err() {
            break;
        }
        let Ok(after) = rustix::fs::fstat(&file) else {
            break;
        };
        if identity(&before) == identity(&after) && bytes.len() as u64 == size {
            let blob =
                gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, &bytes).ok();
            return Some(Sample {
                modified_ms: modified_ms(&before),
                blob,
            });
        }
    }
    let path = OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let fd = crate::confined_path::open_beneath(root, relative, path).ok()?;
    let status = rustix::fs::fstat(&fd).ok()?;
    is_regular(&status).then(|| Sample {
        modified_ms: modified_ms(&status),
        blob: None,
    })
}

/// When each of `paths`, relative to `root`, last changed. `root_fd` is
/// `root`, opened. `None` for a path [`sample`] reads no file at. A file
/// whose bytes HEAD's history gives a commit for shows that commit's time;
/// one whose history the reader did not follow to that commit shows
/// [`LastChange::HistoryUnread`]; any other shows the modification time its
/// sample read. The reader is asked when a sample holds a blob.
pub(crate) fn last_changes(
    history: &History,
    root: &Path,
    root_fd: BorrowedFd<'_>,
    paths: &[&str],
) -> Vec<Option<LastChange>> {
    let samples: Vec<Option<Sample>> = paths.iter().map(|path| sample(root_fd, path)).collect();
    let mut found: Vec<Option<Found>> = vec![None; paths.len()];
    let hashed: Vec<(usize, (&str, gix::ObjectId))> = samples
        .iter()
        .enumerate()
        .filter_map(|(index, sample)| Some((index, (paths[index], sample.as_ref()?.blob?))))
        .collect();
    if !hashed.is_empty()
        && let Ok(status) = rustix::fs::fstat(root_fd)
    {
        let files: Vec<(&str, gix::ObjectId)> = hashed.iter().map(|(_, file)| *file).collect();
        let answers = history.ask(root, (status.st_dev, status.st_ino), &files);
        for ((index, _), answer) in hashed.iter().zip(answers) {
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

    /// The last change of each of `paths`, asked of `history`.
    fn changes_of(history: &History, root: &Path, paths: &[&str]) -> Vec<Option<LastChange>> {
        let fd = std::fs::File::open(root).unwrap();
        last_changes(history, root, fd.as_fd(), paths)
    }

    /// The last change of each of `paths`, asked of a reader of this call's
    /// own.
    fn changes(root: &Path, paths: &[&str]) -> Vec<Option<LastChange>> {
        changes_of(&History::start(), root, paths)
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

    fn blob_of(text: &str) -> gix::ObjectId {
        gix::objs::compute_hash(
            gix::hash::Kind::Sha1,
            gix::objs::Kind::Blob,
            text.as_bytes(),
        )
        .unwrap()
    }

    /// The file that holds the loose object `id`.
    fn loose(root: &Path, id: gix::ObjectId) -> PathBuf {
        let hex = id.to_string();
        root.join(".git/objects").join(&hex[..2]).join(&hex[2..])
    }

    /// A question about `files` of `root`, each a path and its text on
    /// disk, whose pass waits `wait` more, and the channel of its answer.
    fn job(root: &Path, files: &[(&str, &str)], wait: Duration) -> Job {
        let status = std::fs::metadata(root).unwrap();
        Job {
            root: root.to_owned(),
            directory: (status.dev(), status.ino()),
            files: files
                .iter()
                .map(|(path, text)| ((*path).to_owned(), blob_of(text)))
                .collect(),
            until: Instant::now() + wait,
            answer: sync_channel(1).0,
        }
    }

    const DAY: i64 = 86_400;
    const AMPLE: Duration = Duration::from_secs(60);

    /// A file last changed with the newest commit after which HEAD's blob
    /// stayed, whatever later commits changed beside it and whatever the
    /// commit's author time is. A file HEAD does not hold and a file whose
    /// bytes differ from HEAD's blob show the modification time of the
    /// file read, and a directory, a symbolic link, and an absent path show
    /// nothing.
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
        std::os::unix::fs::symlink("notes.txt", root.join("link.txt")).unwrap();
        assert_eq!(
            changes(
                root,
                &[
                    "docs/style.md",
                    "README.md",
                    "tools/check.sh",
                    "notes.txt",
                    "docs",
                    "absent.md",
                    "link.txt"
                ]
            ),
            [
                committed(2 * DAY),
                on_disk(root, "README.md"),
                committed(3 * DAY),
                on_disk(root, "notes.txt"),
                None,
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

    /// A walk reads 256 parents: a file whose bytes 257 ancestors hold
    /// gives no time, and with 256 the walk reads them all and gives the
    /// first commit's.
    #[test]
    fn a_walk_that_would_read_a_257th_parent_gives_no_time() {
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

    /// A commit with the tree `tree`, written with no check of what the
    /// tree names.
    fn commit_of_tree(repo: &gix::Repository, seconds: i64, tree: gix::ObjectId) -> gix::ObjectId {
        let signature = gix::actor::Signature {
            name: "fixture".into(),
            email: "fixture@example.com".into(),
            time: gix::date::Time::new(seconds, 0),
        };
        repo.write_object(gix::objs::Commit {
            tree,
            parents: Default::default(),
            author: signature.clone(),
            committer: signature,
            encoding: None,
            message: "fixture".into(),
            extra_headers: Vec::new(),
        })
        .unwrap()
        .detach()
    }

    /// A walk ends unread at the first parent it cannot read, also when a
    /// later parent holds the same bytes, at a parent whose tree it cannot
    /// read, and at a tree of HEAD's own commit it cannot read. A parent
    /// that introduced the bytes with a later committer time than its child
    /// gives its own time once it is read.
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
        let parent = commit_of_tree(&repo, DAY, tree);
        let child = commit(&repo, 5 * DAY, &[parent], &files);
        check_out(dir.path(), child, &files);
        assert_eq!(
            changes(dir.path(), &["docs/a.md"]),
            [Some(LastChange::HistoryUnread)]
        );

        // HEAD's own commit is read and the subtree of the path is not.
        check_out(dir.path(), parent, &files);
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

    /// A commit object over the tree of `like`, with the parents `parents`,
    /// whose committer line states `time` where a commit states its seconds
    /// and offset.
    fn commit_stating(
        repo: &gix::Repository,
        like: gix::ObjectId,
        parents: &[gix::ObjectId],
        time: &str,
    ) -> gix::ObjectId {
        use gix::objs::Write as _;
        let tree = repo.find_commit(like).unwrap().tree_id().unwrap().detach();
        let mut body = format!("tree {tree}\n");
        for parent in parents {
            body.push_str(&format!("parent {parent}\n"));
        }
        body.push_str("author fixture <fixture@example.com> 1 +0000\n");
        body.push_str(&format!(
            "committer fixture <fixture@example.com> {time}\n\nfixture\n"
        ));
        repo.write_buf(gix::object::Kind::Commit, body.as_bytes())
            .unwrap()
    }

    /// A commit whose committer line states no seconds, or seconds past 64
    /// bits, dates nothing: at HEAD the file shows its modification time,
    /// and as the parent that gave the bytes it leaves the history unread.
    /// A commit that states the second 0 gives that time.
    #[test]
    fn a_committer_time_that_does_not_parse_dates_nothing() {
        let files = [("a.md", "same\n")];
        for time in ["", "99999999999999999999999 +0000"] {
            let (dir, repo) = repository();
            let root = dir.path();
            let plain = commit(&repo, DAY, &[], &files);
            let undated = commit_stating(&repo, plain, &[], time);
            check_out(root, undated, &files);
            let history = History::start();
            assert_eq!(
                changes_of(&history, root, &["a.md"]),
                [on_disk(root, "a.md")],
                "{time:?}"
            );
            let child = commit(&repo, 2 * DAY, &[undated], &files);
            check_out(root, child, &files);
            assert_eq!(
                changes_of(&history, root, &["a.md"]),
                [Some(LastChange::HistoryUnread)],
                "{time:?}"
            );
            // The same reader dates a commit that states its time.
            check_out(root, plain, &files);
            assert_eq!(
                changes_of(&history, root, &["a.md"]),
                [committed(DAY)],
                "{time:?}"
            );
        }
        let (dir, repo) = repository();
        let plain = commit(&repo, DAY, &[], &files);
        let epoch = commit_stating(&repo, plain, &[], "0 +0000");
        check_out(dir.path(), epoch, &files);
        assert_eq!(changes(dir.path(), &["a.md"]), [committed(0)]);
    }

    /// A directory that opens as no repository, a repository with no
    /// commit, a HEAD that names a tag object, and a file past the hashed
    /// size show the modification time. The reader that met the tag
    /// answers for a commit after it.
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
        let history = History::start();
        std::fs::write(root.join(".git/HEAD"), format!("{tag}\n")).unwrap();
        assert_eq!(
            changes_of(&history, root, &["at.txt"]),
            [on_disk(root, "at.txt")]
        );
        std::fs::write(root.join(".git/HEAD"), format!("{only}\n")).unwrap();
        assert_eq!(changes_of(&history, root, &["at.txt"]), [committed(DAY)]);
    }

    /// A HEAD commit, and a parent commit, stored in a pack as a delta that
    /// names itself as its base: the reader ends the read, so the file of
    /// the first shows its modification time and the file of the second no
    /// time. A commit packed whole gives its time, and the reader that met
    /// each delta answers for a commit after it.
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
        let root = dir.path();
        let head = commit(&repo, DAY, &[], &files);
        check_out(root, head, &files);
        pack(root, head, Some(Delta::SelfId));
        let history = History::start();
        assert_eq!(
            changes_of(&history, root, &["a.md"]),
            [on_disk(root, "a.md")]
        );
        let whole = commit(&repo, 3 * DAY, &[], &files);
        check_out(root, whole, &files);
        assert_eq!(changes_of(&history, root, &["a.md"]), [committed(3 * DAY)]);

        let (dir, repo) = repository();
        let root = dir.path();
        let parent = commit(&repo, DAY, &[], &files);
        let head = commit(&repo, 2 * DAY, &[parent], &files);
        check_out(root, head, &files);
        pack(root, parent, Some(Delta::SelfId));
        let history = History::start();
        assert_eq!(
            changes_of(&history, root, &["a.md"]),
            [Some(LastChange::HistoryUnread)]
        );
        check_out(root, whole_in(&repo, &files), &files);
        assert_eq!(changes_of(&history, root, &["a.md"]), [committed(3 * DAY)]);
    }

    /// A commit of `files` with no parent, dated the third day.
    fn whole_in(repo: &gix::Repository, files: &[(&str, &str)]) -> gix::ObjectId {
        commit(repo, 3 * DAY, &[], files)
    }

    /// A repository whose HEAD is a pipe nobody writes blocks the reader and
    /// no pass: the pass waits one second and shows the modification time,
    /// and once four jobs wait beside the one the reader holds a pass waits
    /// for nothing.
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
        let ask = || changes_of(&history, root, &["a.md"]);
        let started = Instant::now();
        assert_eq!(ask(), [on_disk(root, "a.md")]);
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_secs(1) && waited < Duration::from_secs(3),
            "{waited:?}"
        );
        // The reader holds the first job; four more fill the queue, and each
        // of their passes waits its second.
        for _ in 0..4 {
            let started = Instant::now();
            assert_eq!(ask(), [on_disk(root, "a.md")]);
            assert!(started.elapsed() >= Duration::from_secs(1));
        }
        let started = Instant::now();
        assert_eq!(ask(), [on_disk(root, "a.md")]);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
        // No path asks nothing and waits for nothing.
        let started = Instant::now();
        assert!(changes_of(&history, root, &[]).is_empty());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    /// One reader, asked at a commit and then at a later HEAD that holds
    /// the same bytes again after a commit that changed them, gives the
    /// commit that restored them; at a HEAD that no longer holds the path
    /// it gives the time on disk.
    #[test]
    fn a_head_that_moved_is_read_anew() {
        let (dir, repo) = repository();
        let root = dir.path();
        let first = commit(&repo, DAY, &[], &[("a.md", "one\n")]);
        check_out(root, first, &[("a.md", "one\n")]);
        let history = History::start();
        let ask = || changes_of(&history, root, &["a.md"]);
        assert_eq!(ask(), [committed(DAY)]);
        let edited = commit(&repo, 2 * DAY, &[first], &[("a.md", "two\n")]);
        let restored = commit(&repo, 3 * DAY, &[edited], &[("a.md", "one\n")]);
        check_out(root, restored, &[]);
        assert_eq!(ask(), [committed(3 * DAY)]);
        let removed = commit(&repo, 4 * DAY, &[restored], &[("b.md", "b\n")]);
        check_out(root, removed, &[]);
        assert_eq!(ask(), [on_disk(root, "a.md")]);
    }

    /// The reader keeps what a HEAD settles and reads no object for it
    /// again: a commit time and a path HEAD holds other bytes for stand
    /// once the parent commit and HEAD's tree are gone. A blob the reader
    /// holds no answer for is asked of the repository, and an answer an
    /// unreadable object gave is asked again.
    #[test]
    fn the_reader_keeps_the_answers_a_head_settles() {
        let (dir, repo) = repository();
        let root = dir.path();
        let files = [("a.md", "same\n")];
        let parent = commit(&repo, DAY, &[], &files);
        let head = commit(&repo, 2 * DAY, &[parent], &files);
        check_out(root, head, &files);
        let tree = repo.find_commit(head).unwrap().tree_id().unwrap().detach();
        let asked = [("a.md", "same\n"), ("b.md", "untracked\n")];
        let settled = [Some(Found::Committed(DAY * 1_000)), Some(Found::NotHeld)];
        let mut answers = Answers::default();
        assert_eq!(answer(&mut answers, &job(root, &asked, AMPLE)), settled);

        let held_parent = std::fs::read(loose(root, parent)).unwrap();
        std::fs::remove_file(loose(root, parent)).unwrap();
        let held_tree = std::fs::read(loose(root, tree)).unwrap();
        std::fs::remove_file(loose(root, tree)).unwrap();
        assert_eq!(answer(&mut answers, &job(root, &asked, AMPLE)), settled);

        // Other bytes at a path are asked of the repository, whose tree is
        // gone, and the unreadable tree's answer is not kept.
        let other = [("b.md", "other\n")];
        assert_eq!(
            answer(&mut answers, &job(root, &other, AMPLE)),
            [Some(Found::Unread)]
        );
        std::fs::write(loose(root, tree), held_tree).unwrap();
        assert_eq!(
            answer(&mut answers, &job(root, &other, AMPLE)),
            [Some(Found::NotHeld)]
        );

        // A reader with nothing kept meets the absent parent, and reads the
        // parent once it is back.
        let mut answers = Answers::default();
        let one = [("a.md", "same\n")];
        assert_eq!(
            answer(&mut answers, &job(root, &one, AMPLE)),
            [Some(Found::Unread)]
        );
        std::fs::write(loose(root, parent), held_parent).unwrap();
        assert_eq!(
            answer(&mut answers, &job(root, &one, AMPLE)),
            [Some(Found::Committed(DAY * 1_000))]
        );
    }

    /// A job whose pass has stopped waiting is answered nothing and leaves
    /// nothing kept; the same job with time left is answered.
    #[test]
    fn the_reader_reads_no_object_for_a_pass_that_stopped_waiting() {
        let (dir, repo) = repository();
        let root = dir.path();
        let files = [("a.md", "same\n")];
        let head = commit(&repo, DAY, &[], &files);
        check_out(root, head, &files);
        let mut answers = Answers::default();
        assert_eq!(
            answer(&mut answers, &job(root, &files, Duration::ZERO)),
            [None]
        );
        assert!(answers.roots[root].files.is_empty());
        assert_eq!(
            answer(&mut answers, &job(root, &files, AMPLE)),
            [Some(Found::Committed(DAY * 1_000))]
        );
    }

    /// A root whose repository names another directory as its work tree
    /// shows times on disk, also for bytes HEAD holds; the same repository
    /// with its own root as the work tree, and a linked work tree of it,
    /// give the commit's time.
    #[test]
    fn a_root_that_is_not_the_repositorys_work_tree_shows_times_on_disk() {
        let (dir, repo) = repository();
        let root = dir.path();
        let files = [("a.md", "same\n")];
        let head = commit(&repo, DAY, &[], &files);
        check_out(root, head, &files);
        assert_eq!(changes(root, &["a.md"]), [committed(DAY)]);

        // A linked work tree, whose `.git` file names its directory in the
        // repository, is the work tree of that repository.
        let linked = tempfile::tempdir().unwrap();
        let admin = root.join(".git/worktrees/linked");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("HEAD"), format!("{head}\n")).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        let gitfile = linked.path().join(".git");
        std::fs::write(admin.join("gitdir"), format!("{}\n", gitfile.display())).unwrap();
        std::fs::write(&gitfile, format!("gitdir: {}\n", admin.display())).unwrap();
        std::fs::write(linked.path().join("a.md"), "same\n").unwrap();
        assert_eq!(changes(linked.path(), &["a.md"]), [committed(DAY)]);

        let elsewhere = tempfile::tempdir().unwrap();
        let config = root.join(".git/config");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str(&format!(
            "[core]\n\tworktree = {}\n",
            elsewhere.path().display()
        ));
        std::fs::write(&config, text).unwrap();
        assert_eq!(changes(root, &["a.md"]), [on_disk(root, "a.md")]);
    }

    /// The reader keeps 64 roots, the one asked about longest ago leaving
    /// first, and 64 paths of each; another HEAD or another directory at a
    /// root starts the root anew.
    #[test]
    fn the_reader_keeps_64_roots_and_64_paths_of_each() {
        let id = |byte: u8| gix::ObjectId::from_bytes_or_panic(&[byte; 20]);
        let here: Directory = (1, 1);
        let root = |index: usize| PathBuf::from(format!("/r{index}"));
        let mut answers = Answers::default();
        for index in 0..64 {
            keep(
                answers.at(&root(index), here, id(9)),
                "a.md",
                id(1),
                Found::NotHeld,
            );
        }
        // Asking about the first root again makes the second the oldest.
        assert_eq!(answers.at(&root(0), here, id(9)).len(), 1);
        answers.at(&root(64), here, id(9));
        assert_eq!((answers.roots.len(), answers.order.len()), (64, 64));
        assert!(answers.roots.contains_key(&root(0)));
        assert!(!answers.roots.contains_key(&root(1)));
        assert!(answers.roots.contains_key(&root(2)));

        // Another HEAD, and another directory, start a root anew.
        assert_eq!(answers.at(&root(2), here, id(9)).len(), 1);
        assert!(answers.at(&root(2), here, id(8)).is_empty());
        keep(
            answers.at(&root(2), here, id(8)),
            "a.md",
            id(1),
            Found::Unread,
        );
        assert!(answers.at(&root(2), (1, 2), id(8)).is_empty());

        // A path's new blob takes the place of its old one, and the 65th
        // path of a root starts the root anew.
        let files = answers.at(&root(3), here, id(9));
        for index in 1..64 {
            keep(files, &format!("f{index}.md"), id(1), Found::NotHeld);
        }
        assert_eq!(files.len(), 64);
        keep(files, "f1.md", id(4), Found::Committed(8));
        assert_eq!(files.len(), 64);
        assert_eq!(files["f1.md"], (id(4), Found::Committed(8)));
        keep(files, "one-more.md", id(1), Found::NotHeld);
        assert_eq!(files.len(), 1);
    }

    /// Writes `text` to `path` and gives the file the modification time
    /// `seconds` after the epoch.
    fn write_at(path: &Path, text: &str, seconds: u64) {
        std::fs::write(path, text).unwrap();
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }

    /// A reading a write crosses is taken again, and the sample holds the
    /// bytes and the time of the reading that held. A file a write crosses
    /// in both readings, and one that grew past the hashed size during a
    /// reading, give the time of a status taken after and no blob.
    #[test]
    fn a_reading_a_write_crosses_is_taken_again() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::File::open(dir.path()).unwrap();
        let path = dir.path().join("a.md");

        write_at(&path, "one\n", 1_000);
        let mut crossings = 0;
        let sample = sample_between(root.as_fd(), "a.md", &mut || {
            crossings += 1;
            if crossings == 1 {
                write_at(&path, "two\n", 2_000);
            }
        })
        .unwrap();
        assert_eq!(crossings, 2);
        assert_eq!(
            (sample.modified_ms, sample.blob),
            (2_000_000, Some(blob_of("two\n")))
        );

        let mut crossings = 0;
        let sample = sample_between(root.as_fd(), "a.md", &mut || {
            crossings += 1;
            write_at(&path, "new\n", 3_000 + crossings);
        })
        .unwrap();
        assert_eq!(crossings, 2);
        assert_eq!((sample.modified_ms, sample.blob), (3_002_000, None));

        let at_bound = "x".repeat(256 * 1024);
        write_at(&path, &at_bound, 4_000);
        let mut crossings = 0;
        let sample = sample_between(root.as_fd(), "a.md", &mut || {
            crossings += 1;
            write_at(&path, &format!("{at_bound}x"), 5_000);
        })
        .unwrap();
        assert_eq!(crossings, 1);
        assert_eq!((sample.modified_ms, sample.blob), (5_000_000, None));

        // A file that leaves during the reading has no sample.
        write_at(&path, "one\n", 1_000);
        let gone = sample_between(root.as_fd(), "a.md", &mut || {
            let _ = std::fs::remove_file(&path);
            std::fs::write(&path, "again\n").unwrap();
            std::fs::remove_file(&path).unwrap();
        });
        assert!(gone.is_none());
    }

    /// A file that does not open for reading keeps its line: it shows its
    /// modification time, and the reader is asked nothing about it.
    #[test]
    fn a_file_that_does_not_open_for_reading_shows_its_time_on_disk() {
        use std::os::unix::fs::PermissionsExt as _;
        let (dir, repo) = repository();
        let root = dir.path();
        let files = [("a.md", "same\n")];
        let head = commit(&repo, DAY, &[], &files);
        check_out(root, head, &files);
        let path = root.join("a.md");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&path).is_ok() {
            // This user opens a file of any mode, so no mode shows the case.
            return;
        }
        assert_eq!(changes(root, &["a.md"]), [on_disk(root, "a.md")]);
    }
}

//! When a project file last changed, for the lines of the `NAMED_FILES`
//! element.
//!
//! A file whose bytes are the blob HEAD holds for its path last changed
//! with the newest commit after which that blob stayed. The history is
//! read from HEAD through the parent that holds the same blob, as
//! `git log` simplifies the history of one file, and the commit none of
//! whose parents holds the blob is the change. Its committer time is the
//! time the change entered this history: a clone or a checkout writes the
//! file anew and leaves that time as it is. Every other file shows its
//! modification time on disk: a file HEAD holds no blob for, a file whose
//! bytes differ from that blob, a file past [`MAX_HASHED_BYTES`], and a file
//! of a project whose root opens as no Git work tree or whose history
//! cannot be read.

use std::collections::{HashMap, VecDeque};
use std::io::Read as _;
use std::num::NonZeroU64;
use std::os::fd::BorrowedFd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// When a file last changed, in milliseconds since the Unix epoch, as one
/// delivery reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LastChange {
    /// The committer time of the commit that gave the file its bytes.
    Committed(i64),
    /// The committer time of the oldest commit the walk read. The file held
    /// its bytes in that commit, so the change is that old or older.
    CommittedBy(i64),
    /// The modification time of the file on disk.
    OnDisk(i64),
}

/// Parents one file's walk reads. A file that changed more commits ago
/// than this shows the time of the oldest commit read as a bound.
const MAX_PARENTS_READ: usize = 256;
/// The largest commit or tree object a walk decodes.
const MAX_OBJECT_BYTES: NonZeroU64 = NonZeroU64::new(1 << 20).unwrap();
/// The largest file whose bytes are hashed against the blob HEAD holds.
const MAX_HASHED_BYTES: u64 = 256 * 1024;
/// Project roots whose walks the memo holds.
const MEMO_ROOTS: usize = 64;
/// Paths of one root whose walks the memo holds.
const MEMO_FILES: usize = 64;

/// The resident bytes the memo holds at its bounds: for each of
/// [`MEMO_ROOTS`] roots its path and, for each of [`MEMO_FILES`] files, a
/// path of at most 256 bytes, a blob id, and a time.
pub(crate) const RETAINED_BYTES_BOUND: u64 =
    (MEMO_ROOTS as u64) * (8 * 1024 + (MEMO_FILES as u64) * 512);

/// What HEAD's history says of one path: the blob HEAD holds for it and
/// when that blob arrived. `exact` is false for a walk that stopped at its
/// bound or at a parent it could not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tracked {
    blob: gix::ObjectId,
    at_ms: i64,
    exact: bool,
}

/// The walks of one root at one HEAD. `None` is a path HEAD holds no blob
/// for.
struct Walked {
    head: gix::ObjectId,
    files: HashMap<String, Option<Tracked>>,
}

/// The walks of the [`MEMO_ROOTS`] roots read last, so a pass at an
/// unchanged HEAD walks no history.
#[derive(Default)]
struct Memo {
    roots: HashMap<PathBuf, Walked>,
    /// The held roots, the one read longest ago first.
    order: VecDeque<PathBuf>,
}

static MEMO: Mutex<Option<Memo>> = Mutex::new(None);

impl Memo {
    fn held(&self, root: &Path, head: gix::ObjectId, path: &str) -> Option<Option<Tracked>> {
        let walked = self.roots.get(root).filter(|walked| walked.head == head)?;
        walked.files.get(path).copied()
    }

    /// Holds the walk of `path` at `head`, and makes `root` the root read
    /// last. A root at another HEAD, and a root at its file bound, starts
    /// anew.
    fn hold(&mut self, root: &Path, head: gix::ObjectId, path: &str, tracked: Option<Tracked>) {
        self.order.retain(|held| held != root);
        if self.order.len() == MEMO_ROOTS
            && let Some(oldest) = self.order.pop_front()
        {
            self.roots.remove(&oldest);
        }
        self.order.push_back(root.to_owned());
        let walked = self.roots.entry(root.to_owned()).or_insert_with(|| Walked {
            head,
            files: HashMap::new(),
        });
        if walked.head != head || walked.files.len() == MEMO_FILES {
            walked.head = head;
            walked.files.clear();
        }
        walked.files.insert(path.to_owned(), tracked);
    }
}

fn millis(seconds: i64) -> i64 {
    seconds.saturating_mul(1_000)
}

/// The blob `commit` holds at `path`: a regular or executable file. `Ok(None)`
/// for a path the commit holds nothing or another kind of entry at, and
/// `Err` for a commit or tree that cannot be read.
fn blob_at(commit: &gix::Commit<'_>, path: &str) -> Result<Option<gix::ObjectId>, ()> {
    use gix::objs::tree::EntryKind;
    let entry = commit
        .tree()
        .map_err(drop)?
        .lookup_entry_by_path(path)
        .map_err(drop)?;
    Ok(entry
        .filter(|entry| {
            matches!(
                entry.mode().kind(),
                EntryKind::Blob | EntryKind::BlobExecutable
            )
        })
        .map(|entry| entry.object_id()))
}

/// What the history of `head` says of `path`. The walk starts at `head` and
/// moves to the first parent that holds the blob `head` holds, reading
/// [`MAX_PARENTS_READ`] parents at most. It ends exact at a commit none of
/// whose parents holds the blob, and as a bound at the commit where it
/// reached its limit or met a parent it could not read, as a shallow clone
/// has. `None` when `head` holds no blob at `path` or cannot be read.
fn walk(repo: &gix::Repository, head: gix::ObjectId, path: &str) -> Option<Tracked> {
    let mut commit = repo.find_commit(head).ok()?;
    let blob = blob_at(&commit, path).ok()??;
    let tracked = |commit: &gix::Commit<'_>, exact: bool| {
        Some(Tracked {
            blob,
            at_ms: millis(commit.time().ok()?.seconds),
            exact,
        })
    };
    let mut read = 0;
    loop {
        let mut same = None;
        let mut unread = false;
        for parent in commit.parent_ids() {
            if read == MAX_PARENTS_READ {
                return tracked(&commit, false);
            }
            read += 1;
            let Ok(parent) = repo.find_commit(parent.detach()) else {
                unread = true;
                continue;
            };
            match blob_at(&parent, path) {
                Ok(Some(held)) if held == blob => {
                    same = Some(parent);
                    break;
                }
                Ok(_) => {}
                Err(()) => unread = true,
            }
        }
        match same {
            Some(parent) => commit = parent,
            None => return tracked(&commit, !unread),
        }
    }
}

/// HEAD of the repository whose work tree is `root`.
fn open(root: &Path) -> Option<(gix::Repository, gix::ObjectId)> {
    let repo = crate::git_sources::open(root, MAX_OBJECT_BYTES).ok()?;
    repo.workdir()?;
    let head = repo.head_id().ok()?.detach();
    Some((repo, head))
}

/// Whether the file at `relative` beneath `root` is a regular file of at
/// most [`MAX_HASHED_BYTES`] whose bytes hash to `blob`.
fn holds_blob(root: BorrowedFd<'_>, relative: &str, blob: gix::ObjectId) -> bool {
    use rustix::fs::{FileType, OFlags};
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let Ok(fd) = crate::confined_path::open_beneath(root, relative, flags) else {
        return false;
    };
    let Ok(stat) = rustix::fs::fstat(&fd) else {
        return false;
    };
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || u64::try_from(stat.st_size).map_or(true, |size| size > MAX_HASHED_BYTES)
    {
        return false;
    }
    let mut bytes = Vec::new();
    // One byte past the bound shows a file that grew since its size was read.
    if std::fs::File::from(fd)
        .take(MAX_HASHED_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_HASHED_BYTES
    {
        return false;
    }
    gix::objs::compute_hash(blob.kind(), gix::objs::Kind::Blob, &bytes).is_ok_and(|id| id == blob)
}

/// When each of `files` last changed. A file is a path relative to `root`
/// and the modification time of the regular file there, in milliseconds.
/// `root_fd` is `root`, opened.
pub(crate) fn last_changes(
    root: &Path,
    root_fd: BorrowedFd<'_>,
    files: &[(&str, i64)],
) -> Vec<LastChange> {
    let history = open(root);
    files
        .iter()
        .map(|(path, modified_ms)| {
            let tracked = history.as_ref().and_then(|(repo, head)| {
                let held = MEMO
                    .lock()
                    .expect("named file change memo mutex")
                    .get_or_insert_default()
                    .held(root, *head, path);
                held.unwrap_or_else(|| {
                    let tracked = walk(repo, *head, path);
                    MEMO.lock()
                        .expect("named file change memo mutex")
                        .get_or_insert_default()
                        .hold(root, *head, path, tracked);
                    tracked
                })
            });
            match tracked {
                Some(tracked) if holds_blob(root_fd, path, tracked.blob) => {
                    if tracked.exact {
                        LastChange::Committed(tracked.at_ms)
                    } else {
                        LastChange::CommittedBy(tracked.at_ms)
                    }
                }
                _ => LastChange::OnDisk(*modified_ms),
            }
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

    fn changes(root: &Path, paths: &[&str]) -> Vec<LastChange> {
        let fd = std::fs::File::open(root).unwrap();
        let files: Vec<(&str, i64)> = paths.iter().map(|path| (*path, 77_000)).collect();
        last_changes(root, fd.as_fd(), &files)
    }

    const DAY: i64 = 86_400;

    /// A file last changed with the newest commit after which HEAD's blob
    /// stayed, whatever later commits changed beside it and whatever the
    /// commit's author time is. A file HEAD does not hold, a file whose
    /// bytes differ from HEAD's blob, and a directory show the modification
    /// time the caller read.
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
                    "docs"
                ]
            ),
            [
                LastChange::Committed(2 * DAY * 1_000),
                LastChange::OnDisk(77_000),
                LastChange::Committed(3 * DAY * 1_000),
                LastChange::OnDisk(77_000),
                LastChange::OnDisk(77_000),
            ]
        );
        // The same HEAD is answered from the memo, and a new HEAD is walked.
        assert_eq!(
            changes(root, &["docs/style.md"]),
            [LastChange::Committed(2 * DAY * 1_000)]
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
            [
                LastChange::Committed(4 * DAY * 1_000),
                LastChange::OnDisk(77_000)
            ]
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
            [
                LastChange::Committed(2 * DAY * 1_000),
                LastChange::Committed(4 * DAY * 1_000)
            ]
        );
    }

    /// A walk reads 256 parents. A file unchanged over more commits than
    /// that, and a file whose history ends at a commit with an absent
    /// parent, show the time of the oldest commit read as a bound.
    #[test]
    fn a_walk_that_ends_at_its_bound_or_a_missing_parent_gives_a_bound() {
        let (dir, repo) = repository();
        let root = dir.path();
        let files = [("a.md", "same\n")];
        let mut tip = commit(&repo, DAY, &[], &files);
        for index in 0..=MAX_PARENTS_READ as i64 {
            tip = commit(&repo, DAY + 1 + index, &[tip], &files);
        }
        check_out(root, tip, &files);
        // The tip is at DAY + 257. The walk reads the 256 commits at
        // DAY + 256 down to DAY + 1 and stops at the last of them, with the
        // first commit unread.
        assert_eq!(
            changes(root, &["a.md"]),
            [LastChange::CommittedBy((DAY + 1) * 1_000)]
        );
        let one_fewer = repo.find_commit(tip).unwrap().parent_ids().next().unwrap();
        check_out(root, one_fewer.detach(), &files);
        assert_eq!(
            changes(root, &["a.md"]),
            [LastChange::Committed(DAY * 1_000)]
        );

        let (dir, repo) = repository();
        let root = dir.path();
        let absent = gix::ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
        let shallow = commit(&repo, 5 * DAY, &[absent], &files);
        check_out(root, shallow, &files);
        assert_eq!(
            changes(root, &["a.md"]),
            [LastChange::CommittedBy(5 * DAY * 1_000)]
        );
    }

    /// A directory that opens as no repository, a repository with no
    /// commit, and a file past the hashed size show the modification time.
    #[test]
    fn a_file_without_readable_history_shows_its_modification_time() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "text\n").unwrap();
        assert_eq!(changes(dir.path(), &["a.md"]), [LastChange::OnDisk(77_000)]);

        let (dir, repo) = repository();
        let root = dir.path();
        std::fs::write(root.join("a.md"), "text\n").unwrap();
        assert_eq!(changes(root, &["a.md"]), [LastChange::OnDisk(77_000)]);

        let at_bound = "x".repeat(MAX_HASHED_BYTES as usize);
        let past_bound = format!("{at_bound}x");
        let files = [("at.txt", at_bound.as_str()), ("past.txt", &past_bound)];
        let only = commit(&repo, DAY, &[], &files);
        check_out(root, only, &files);
        assert_eq!(
            changes(root, &["at.txt", "past.txt"]),
            [
                LastChange::Committed(DAY * 1_000),
                LastChange::OnDisk(77_000)
            ]
        );
    }

    /// The memo holds 64 roots and 64 paths of each, and answers for the
    /// HEAD it walked.
    #[test]
    fn the_memo_holds_64_roots_and_64_paths_at_one_head() {
        let id = |byte: u8| gix::ObjectId::from_bytes_or_panic(&[byte; 20]);
        let tracked = Some(Tracked {
            blob: id(9),
            at_ms: 5,
            exact: true,
        });
        let mut memo = Memo::default();
        for index in 0..MEMO_ROOTS {
            memo.hold(Path::new(&format!("/r{index}")), id(1), "a.md", tracked);
        }
        memo.hold(Path::new("/r0"), id(1), "b.md", None);
        memo.hold(Path::new("/r64"), id(1), "a.md", tracked);
        assert_eq!((memo.roots.len(), memo.order.len()), (64, 64));
        assert_eq!(memo.held(Path::new("/r0"), id(1), "a.md"), Some(tracked));
        assert_eq!(memo.held(Path::new("/r0"), id(1), "b.md"), Some(None));
        assert_eq!(memo.held(Path::new("/r1"), id(1), "a.md"), None);
        assert_eq!(memo.held(Path::new("/r0"), id(2), "a.md"), None);
        // Another HEAD, and the 65th path, start the root anew.
        memo.hold(Path::new("/r0"), id(2), "c.md", tracked);
        assert_eq!(memo.held(Path::new("/r0"), id(2), "a.md"), None);
        for index in 1..MEMO_FILES {
            memo.hold(Path::new("/r2"), id(1), &format!("f{index}.md"), tracked);
        }
        assert_eq!(memo.roots[Path::new("/r2")].files.len(), 64);
        memo.hold(Path::new("/r2"), id(1), "one-more.md", tracked);
        assert_eq!(memo.roots[Path::new("/r2")].files.len(), 1);
    }
}

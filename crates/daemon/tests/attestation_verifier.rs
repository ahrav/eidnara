//! The attestation verifier's core over bare fixture repositories with no worktree: each verdict and
//! reason, the header gate and the one blob read, the deadline polls, and identical verdicts across
//! a worktree edit, a HEAD move, and a repack.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

mod support;

use daemon::attestation_verifier::{
    Claim, Deadline, Digests, EntryClass, Evaluation, InvalidClaimPath, MAX_BLOB_BYTES,
    ObjectClass, Observed, Reason, Stage, Verdict, header_verdict, verdict, verify,
};
use kernel::applicability::EvalBudget;
use sha2::{Digest as _, Sha256};

const DOC: &[u8] = b"# Architecture\n\nThe daemon owns the store.\n";

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn claim(path: &str, bytes: &[u8], oid: &str) -> Claim {
    Claim::new(path, sha256(bytes), bytes.len() as u64, oid).unwrap()
}

fn unbounded() -> EvalBudget {
    EvalBudget::unbounded()
}

/// A bare repository driven through git plumbing with fixed identities and times.
struct Bare {
    root: PathBuf,
}

impl Bare {
    fn init(root: &Path) -> Self {
        std::fs::create_dir_all(root).unwrap();
        let bare = Self {
            root: root.to_path_buf(),
        };
        bare.git(
            &["init", "--bare", "--quiet", "--initial-branch=main"],
            None,
        );
        bare
    }

    fn git(&self, args: &[&str], stdin: Option<&[u8]>) -> String {
        use std::io::Write as _;
        // The cleared environment keeps `GIT_DIR` and other repository variables of the caller
        // from redirecting the fixture's commands to another repository.
        let mut child = Command::new("git")
            .args(["-c", "core.autocrlf=false"])
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_DATE", "@1 +0000")
            .env("GIT_COMMITTER_DATE", "@1 +0000")
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.unwrap_or_default())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn blob(&self, bytes: &[u8]) -> String {
        self.git(&["hash-object", "-w", "--stdin"], Some(bytes))
    }

    /// A tree of `(mode, kind, oid, name)` entries; `--missing` admits a gitlink whose commit this
    /// repository does not hold.
    fn tree(&self, entries: &[(&str, &str, &str, &str)]) -> String {
        let listing: String = entries
            .iter()
            .map(|(mode, kind, oid, name)| format!("{mode} {kind} {oid}\t{name}\n"))
            .collect();
        self.git(&["mktree", "--missing"], Some(listing.as_bytes()))
    }

    fn commit(&self, tree: &str, parent: Option<&str>, message: &str) -> String {
        let mut args = vec!["commit-tree", tree, "-m", message];
        if let Some(parent) = parent {
            args.extend(["-p", parent]);
        }
        self.git(&args, None)
    }

    /// A commit whose tree holds `doc.md` with `bytes` and nothing else.
    fn commit_doc(&self, bytes: &[u8], parent: Option<&str>) -> String {
        let blob = self.blob(bytes);
        let tree = self.tree(&[("100644", "blob", &blob, "doc.md")]);
        self.commit(&tree, parent, "doc")
    }

    /// Writes `bytes` as a loose object of `kind` without validating them.
    fn literal(&self, kind: &str, bytes: &[u8]) -> String {
        self.git(
            &["hash-object", "-t", kind, "--literally", "-w", "--stdin"],
            Some(bytes),
        )
    }

    fn loose_path(&self, oid: &str) -> PathBuf {
        self.root.join("objects").join(&oid[..2]).join(&oid[2..])
    }

    fn delete_loose(&self, oid: &str) {
        std::fs::remove_file(self.loose_path(oid)).unwrap();
    }

    /// Replaces `oid`'s loose file with the loose file of `other`, a valid object stored under the
    /// wrong id.
    fn store_under(&self, oid: &str, other: &str) {
        let stored = std::fs::read(self.loose_path(other)).unwrap();
        self.replace_loose(oid, &stored);
    }

    /// Replaces `oid`'s loose file with `stored`, written as-is.
    fn replace_loose(&self, oid: &str, stored: &[u8]) {
        let path = self.loose_path(oid);
        std::fs::remove_file(&path).unwrap();
        std::fs::write(path, stored).unwrap();
    }

    /// Every file under the repository with the sha256 of its bytes.
    fn snapshot(&self) -> BTreeMap<PathBuf, [u8; 32]> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, [u8; 32]>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.insert(path.clone(), sha256(&std::fs::read(&path).unwrap()));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.root, &mut out);
        out
    }
}

fn fixture(dir: &Path) -> Bare {
    Bare::init(&dir.join("repo.git"))
}

fn unavailable(reason: Reason) -> Evaluation {
    Evaluation {
        verdict: Verdict::Unavailable(reason),
        bytes_read: 0,
    }
}

/// A deadline that has passed exactly at the poll before `at`, recording every poll it answers.
struct PassesAt {
    at: Option<Stage>,
    polls: RefCell<Vec<Stage>>,
}

impl PassesAt {
    fn new(at: Option<Stage>) -> Self {
        Self {
            at,
            polls: RefCell::new(Vec::new()),
        }
    }
}

impl Deadline for PassesAt {
    fn passed(&self, stage: Stage) -> bool {
        self.polls.borrow_mut().push(stage);
        self.at == Some(stage)
    }
}

#[test]
fn the_fixture_has_no_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let repo = gix::open_opts(&bare.root, gix::open::Options::isolated()).unwrap();
    assert!(repo.is_bare());
    assert!(repo.workdir().is_none());
    assert!(!bare.root.join("doc.md").exists());
}

#[test]
fn committed_bytes_match_and_a_later_commit_mismatches() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let first = bare.commit_doc(DOC, None);
    let changed = b"# Architecture\n\nThe plugin owns the store.\n";
    assert_eq!(changed.len(), DOC.len());
    let second = bare.commit_doc(changed, Some(&first));

    let polls = PassesAt::new(None);
    assert_eq!(
        verify(&bare.root, &claim("doc.md", DOC, &first), &polls),
        Evaluation {
            verdict: Verdict::Match,
            bytes_read: DOC.len() as u64,
        }
    );
    // One header poll and one blob poll: the claim reads one blob.
    assert_eq!(
        *polls.polls.borrow(),
        [
            Stage::Open,
            Stage::Commit,
            Stage::Tree,
            Stage::Header,
            Stage::Blob
        ]
    );
    assert_eq!(
        verify(&bare.root, &claim("doc.md", DOC, &second), &unbounded()),
        Evaluation {
            verdict: Verdict::Mismatch,
            bytes_read: DOC.len() as u64,
        }
    );
    assert_eq!(
        verify(&bare.root, &claim("doc.md", changed, &second), &unbounded()).verdict,
        Verdict::Match
    );
}

#[test]
fn a_nested_path_walks_one_tree_per_component() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let blob = bare.blob(DOC);
    let guide = bare.tree(&[("100755", "blob", &blob, "a.md")]);
    let docs = bare.tree(&[("040000", "tree", &guide, "guide")]);
    let root = bare.tree(&[("040000", "tree", &docs, "docs")]);
    let commit = bare.commit(&root, None, "nested");
    let polls = PassesAt::new(None);
    assert_eq!(
        verify(&bare.root, &claim("docs/guide/a.md", DOC, &commit), &polls).verdict,
        Verdict::Match
    );
    let trees = polls
        .polls
        .borrow()
        .iter()
        .filter(|stage| **stage == Stage::Tree)
        .count();
    assert_eq!(trees, 3);
}

#[test]
fn line_endings_normalize_in_either_direction() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let lf = bare.commit_doc(b"one\ntwo\n", None);
    assert_eq!(
        verify(
            &bare.root,
            &claim("doc.md", b"one\r\ntwo\r\n", &lf),
            &unbounded()
        ),
        Evaluation {
            verdict: Verdict::Match,
            bytes_read: 8,
        }
    );
    let crlf = bare.commit_doc(b"one\r\ntwo\r\n", None);
    assert_eq!(
        verify(
            &bare.root,
            &claim("doc.md", b"one\ntwo\n", &crlf),
            &unbounded()
        )
        .verdict,
        Verdict::Match
    );
    let mixed = bare.commit_doc(b"one\r\ntwo\n", None);
    assert_eq!(
        verify(
            &bare.root,
            &claim("doc.md", b"one\r\ntwo\r\n", &mixed),
            &unbounded()
        )
        .verdict,
        Verdict::Match
    );
    // A conversion that changes other bytes is no line-ending form.
    assert_eq!(
        verify(
            &bare.root,
            &claim("doc.md", b"one\r\ntwo\r\r", &lf),
            &unbounded()
        )
        .verdict,
        Verdict::Mismatch
    );
}

#[test]
fn entries_that_hold_no_blob_are_named_by_kind() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let blob = bare.blob(DOC);
    let target = bare.blob(b"doc.md");
    let sub = bare.tree(&[("100644", "blob", &blob, "inner.md")]);
    let gitlink = "1111111111111111111111111111111111111111";
    let root = bare.tree(&[
        ("040000", "tree", &sub, "dir"),
        ("100644", "blob", &blob, "doc.md"),
        ("120000", "blob", &target, "link"),
        ("160000", "commit", gitlink, "module"),
    ]);
    let commit = bare.commit(&root, None, "kinds");
    let check = |path: &str| verify(&bare.root, &claim(path, DOC, &commit), &unbounded());
    assert_eq!(check("dir/inner.md").verdict, Verdict::Match);
    assert_eq!(check("absent.md"), unavailable(Reason::PathAbsent));
    assert_eq!(check("dir/absent.md"), unavailable(Reason::PathAbsent));
    assert_eq!(check("doc.md/beneath"), unavailable(Reason::PathAbsent));
    assert_eq!(
        check("dir"),
        unavailable(Reason::NotABlob(EntryClass::Tree))
    );
    assert_eq!(
        check("link"),
        unavailable(Reason::NotABlob(EntryClass::Symlink))
    );
    assert_eq!(
        check("link/beneath"),
        unavailable(Reason::NotABlob(EntryClass::Symlink))
    );
    assert_eq!(
        check("module"),
        unavailable(Reason::NotABlob(EntryClass::Gitlink))
    );
    assert_eq!(
        check("module/beneath/file.md"),
        unavailable(Reason::NotABlob(EntryClass::Gitlink))
    );
}

#[test]
fn missing_or_corrupt_objects_are_unreadable_never_absent() {
    let dir = tempfile::tempdir().unwrap();
    let cases: [(&str, ObjectClass, bool); 6] = [
        ("commit", ObjectClass::Commit, false),
        ("commit", ObjectClass::Commit, true),
        ("tree", ObjectClass::Tree, false),
        ("tree", ObjectClass::Tree, true),
        ("blob", ObjectClass::Blob, false),
        ("blob", ObjectClass::Blob, true),
    ];
    for (index, (kind, class, corrupted)) in cases.into_iter().enumerate() {
        let bare = Bare::init(&dir.path().join(format!("repo-{index}.git")));
        let blob = bare.blob(DOC);
        let tree = bare.tree(&[("100644", "blob", &blob, "doc.md")]);
        let commit = bare.commit(&tree, None, "doc");
        // A valid object of the same kind that would read as a mismatch or an absent path: a blob
        // of the claimed length, a tree without `doc.md`, and a commit of that tree.
        let other_blob = bare.blob(&vec![b'z'; DOC.len()]);
        let other_tree = bare.tree(&[("100644", "blob", &blob, "other.md")]);
        let other_commit = bare.commit(&other_tree, None, "other");
        let (target, other) = match class {
            ObjectClass::Commit => (&commit, &other_commit),
            ObjectClass::Tree => (&tree, &other_tree),
            ObjectClass::Blob => (&blob, &other_blob),
        };
        if corrupted {
            // The object decodes, so only the hash check rejects it.
            bare.store_under(target, other);
        } else {
            bare.delete_loose(target);
        }
        // A rejected blob counts the bytes decoded before the hash check.
        let bytes_read = if corrupted && class == ObjectClass::Blob {
            DOC.len() as u64
        } else {
            0
        };
        assert_eq!(
            verify(&bare.root, &claim("doc.md", DOC, &commit), &unbounded()),
            Evaluation {
                verdict: Verdict::Unavailable(Reason::ObjectUnreadable(class)),
                bytes_read,
            },
            "{kind} corrupted={corrupted}"
        );
    }

    // A commit and a tree whose bytes hash to their ids but do not decode are unreadable.
    let bare = Bare::init(&dir.path().join("literal.git"));
    let literal = |kind: &str, bytes: &[u8]| bare.literal(kind, bytes);
    let malformed_tree = literal("tree", b"100644 doc.md");
    let commit = bare.commit(&malformed_tree, None, "malformed tree");
    assert_eq!(
        verify(&bare.root, &claim("doc.md", DOC, &commit), &unbounded()),
        unavailable(Reason::ObjectUnreadable(ObjectClass::Tree))
    );
    // A commit or tree whose first parsed field is valid but whose remainder is malformed.
    let blob = bare.blob(DOC);
    let blob_id = gix::ObjectId::from_hex(blob.as_bytes()).unwrap();
    let mut suffixed_tree = b"100644 doc.md\0".to_vec();
    suffixed_tree.extend_from_slice(blob_id.as_slice());
    suffixed_tree.extend_from_slice(b"100644 z.md");
    let commit = bare.commit(&literal("tree", &suffixed_tree), None, "suffixed tree");
    assert_eq!(
        verify(&bare.root, &claim("doc.md", DOC, &commit), &unbounded()),
        unavailable(Reason::ObjectUnreadable(ObjectClass::Tree))
    );
    let tree = bare.tree(&[("100644", "blob", &blob, "doc.md")]);
    let suffixed_commit = literal("commit", format!("tree {tree}\nno signature\n").as_bytes());
    assert_eq!(
        verify(
            &bare.root,
            &claim("doc.md", DOC, &suffixed_commit),
            &unbounded()
        ),
        unavailable(Reason::ObjectUnreadable(ObjectClass::Commit))
    );
    let malformed_commit = literal("commit", b"no tree header\n");
    assert_eq!(
        verify(
            &bare.root,
            &claim("doc.md", DOC, &malformed_commit),
            &unbounded()
        ),
        unavailable(Reason::ObjectUnreadable(ObjectClass::Commit))
    );

    // A blob whose stored bytes are not zlib data is unreadable at its header.
    let bare = Bare::init(&dir.path().join("garbage.git"));
    let commit = bare.commit_doc(DOC, None);
    let blob = bare.blob(DOC);
    bare.replace_loose(&blob, b"not zlib");
    assert_eq!(
        verify(&bare.root, &claim("doc.md", DOC, &commit), &unbounded()),
        unavailable(Reason::ObjectUnreadable(ObjectClass::Blob))
    );

    // An oid that names a blob, or does not parse, names no readable commit.
    let parsed_blob = bare.blob(b"other");
    for oid in [parsed_blob.as_str(), "not-an-oid", &"A".repeat(40)] {
        assert_eq!(
            verify(&bare.root, &claim("doc.md", DOC, oid), &unbounded()),
            unavailable(Reason::ObjectUnreadable(ObjectClass::Commit)),
            "{oid}"
        );
    }
}

#[test]
fn a_directory_that_is_no_repository_is_unopenable() {
    let dir = tempfile::tempdir().unwrap();
    let oid = "1111111111111111111111111111111111111111";
    assert_eq!(
        verify(dir.path(), &claim("doc.md", DOC, oid), &unbounded()),
        unavailable(Reason::RepositoryUnopenable)
    );
}

#[test]
fn the_driver_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let commit = bare.commit_doc(DOC, None);
    let before = bare.snapshot();
    for path in ["doc.md", "absent.md", "doc.md/x"] {
        verify(&bare.root, &claim(path, DOC, &commit), &unbounded());
    }
    verify(
        &bare.root,
        &claim("doc.md", b"other", &commit),
        &unbounded(),
    );
    assert_eq!(bare.snapshot(), before);
    // The snapshot observes a write to the object store.
    bare.blob(b"a new object");
    assert_ne!(bare.snapshot(), before);
}

#[test]
fn replacement_refs_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let commit = bare.commit_doc(DOC, None);
    let replaced = b"# Architecture\n\nThe plugin owns the store.\n";
    let other_commit = bare.commit_doc(replaced, None);
    bare.git(&["replace", &bare.blob(DOC), &bare.blob(replaced)], None);
    bare.git(&["replace", &commit, &other_commit], None);
    // Git itself honors both replacements.
    let shown = bare.git(&["cat-file", "-p", &format!("{commit}:doc.md")], None);
    assert_eq!(format!("{shown}\n").as_bytes(), replaced);
    // gix loads `refs/replace` only when `core.useReplaceRefs` is `false`; with it set, a plain
    // isolated gix open reads the replacement.
    bare.git(&["config", "core.useReplaceRefs", "false"], None);
    let plain = gix::open_opts(&bare.root, gix::open::Options::isolated()).unwrap();
    let id = gix::ObjectId::from_hex(bare.blob(DOC).as_bytes()).unwrap();
    assert_eq!(
        plain.find_object(id).unwrap().data.as_slice(),
        &replaced[..]
    );
    assert_eq!(
        verify(&bare.root, &claim("doc.md", DOC, &commit), &unbounded()).verdict,
        Verdict::Match
    );
    assert_eq!(
        verify(
            &bare.root,
            &claim("doc.md", replaced, &commit),
            &unbounded()
        )
        .verdict,
        Verdict::Mismatch
    );
}

#[test]
fn the_header_gates_every_blob_read() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let at_budget = vec![b'x'; MAX_BLOB_BYTES as usize];
    let over_budget = vec![b'x'; MAX_BLOB_BYTES as usize + 1];
    let at = bare.commit_doc(&at_budget, None);
    let over = bare.commit_doc(&over_budget, None);

    assert_eq!(
        verify(&bare.root, &claim("doc.md", &at_budget, &at), &unbounded()),
        Evaluation {
            verdict: Verdict::Match,
            bytes_read: MAX_BLOB_BYTES,
        }
    );
    let polls = PassesAt::new(None);
    assert_eq!(
        verify(&bare.root, &claim("doc.md", &over_budget, &over), &polls),
        unavailable(Reason::OverBudget)
    );
    assert!(!polls.polls.borrow().contains(&Stage::Blob));

    // A header size no line-ending form can reach decides the mismatch before any read.
    let doc = bare.commit_doc(DOC, None);
    let long = DOC.repeat(3);
    let polls = PassesAt::new(None);
    assert_eq!(
        verify(&bare.root, &claim("doc.md", &long, &doc), &polls),
        Evaluation {
            verdict: Verdict::Mismatch,
            bytes_read: 0,
        }
    );
    assert!(!polls.polls.borrow().contains(&Stage::Blob));
    let short = &DOC[..DOC.len() / 2 - 1];
    assert_eq!(
        verify(&bare.root, &claim("doc.md", short, &doc), &unbounded()),
        Evaluation {
            verdict: Verdict::Mismatch,
            bytes_read: 0,
        }
    );
    // A length a line-ending form could reach reads the blob and compares its digests.
    assert_eq!(
        verify(&bare.root, &claim("doc.md", &DOC[1..], &doc), &unbounded()),
        Evaluation {
            verdict: Verdict::Mismatch,
            bytes_read: DOC.len() as u64,
        }
    );
}

#[test]
fn a_blob_whose_delta_base_exceeds_the_allocation_limit_is_over_budget() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let base = vec![b'b'; MAX_BLOB_BYTES as usize + 1024];
    let (_, small) = support::git_repo::write_delta_pack(
        &bare.root.join("objects"),
        gix::objs::Kind::Blob,
        &base,
        DOC,
    );
    let small = small.to_string();
    let tree = bare.tree(&[("100644", "blob", &small, "doc.md")]);
    let commit = bare.commit(&tree, None, "delta");
    let polls = PassesAt::new(None);
    assert_eq!(
        verify(&bare.root, &claim("doc.md", DOC, &commit), &polls),
        unavailable(Reason::OverBudget)
    );
    // The header passed the gate, so the refusal came from the blob read.
    assert_eq!(polls.polls.borrow().last(), Some(&Stage::Blob));

    // The same delta against a base inside the limit reads back and matches.
    let roomy = Bare::init(&dir.path().join("roomy.git"));
    let (_, small) = support::git_repo::write_delta_pack(
        &roomy.root.join("objects"),
        gix::objs::Kind::Blob,
        &vec![b'b'; 1024],
        DOC,
    );
    let tree = roomy.tree(&[("100644", "blob", &small.to_string(), "doc.md")]);
    let commit = roomy.commit(&tree, None, "delta");
    assert_eq!(
        verify(&roomy.root, &claim("doc.md", DOC, &commit), &unbounded()).verdict,
        Verdict::Match
    );
}

/// The commit's, tree's, and blob's bytes, each addressed by its id.
fn doc_objects() -> [(ObjectClass, &'static str, Vec<u8>); 3] {
    let id =
        |kind, bytes: &[u8]| gix::objs::compute_hash(gix::hash::Kind::Sha1, kind, bytes).unwrap();
    let blob = id(gix::objs::Kind::Blob, DOC);
    let mut tree = b"100644 doc.md\0".to_vec();
    tree.extend_from_slice(blob.as_slice());
    let tree_id = id(gix::objs::Kind::Tree, &tree);
    let commit = format!(
        "tree {tree_id}\nauthor f <f@e.invalid> 1 +0000\ncommitter f <f@e.invalid> 1 +0000\n\ndoc\n"
    )
    .into_bytes();
    [
        (ObjectClass::Commit, "commit", commit),
        (ObjectClass::Tree, "tree", tree),
        (ObjectClass::Blob, "blob", DOC.to_vec()),
    ]
}

#[test]
fn a_delta_chain_that_revisits_an_entry_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    for (index, (class, _, _)) in doc_objects().into_iter().enumerate() {
        let bare = Bare::init(&dir.path().join(format!("repo-{index}.git")));
        let mut commit = String::new();
        for (stored, kind, bytes) in doc_objects() {
            let id = if stored == class {
                let kind = gix::objs::Kind::from_bytes(kind.as_bytes()).unwrap();
                let id = gix::objs::compute_hash(gix::hash::Kind::Sha1, kind, &bytes).unwrap();
                support::git_repo::write_self_delta_pack(&bare.root.join("objects"), id, &bytes);
                id.to_string()
            } else {
                bare.literal(kind, &bytes)
            };
            if stored == ObjectClass::Commit {
                commit = id;
            }
        }
        assert_eq!(
            verify_within(&bare.root, &claim("doc.md", DOC, &commit)),
            unavailable(Reason::ObjectUnreadable(class)),
            "{class:?}"
        );
    }
}

#[test]
fn a_passed_deadline_opens_no_repository() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-repository-here");
    let oid = "1111111111111111111111111111111111111111";
    // With time left the same root is unopenable, so a deadline verdict proves the open never ran.
    assert_eq!(
        verify(&missing, &claim("doc.md", DOC, oid), &unbounded()),
        unavailable(Reason::RepositoryUnopenable)
    );
    let cancelled = unbounded();
    cancelled.cancel();
    assert_eq!(
        verify(&missing, &claim("doc.md", DOC, oid), &cancelled),
        unavailable(Reason::Deadline(Stage::Open))
    );
    let expired = EvalBudget::new(
        Some(std::time::Instant::now()),
        Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(
        verify(&missing, &claim("doc.md", DOC, oid), &expired),
        unavailable(Reason::Deadline(Stage::Open))
    );
}

#[test]
fn a_deadline_before_the_header_poll_reads_no_blob() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let commit = bare.commit_doc(DOC, None);
    let doc = claim("doc.md", DOC, &commit);
    for stage in [Stage::Commit, Stage::Tree, Stage::Header, Stage::Blob] {
        let polls = PassesAt::new(Some(stage));
        assert_eq!(
            verify(&bare.root, &doc, &polls),
            unavailable(Reason::Deadline(stage))
        );
        assert_eq!(polls.polls.borrow().last(), Some(&stage));
    }
    assert_eq!(
        verify(&bare.root, &doc, &unbounded()).bytes_read,
        DOC.len() as u64
    );
}

#[test]
fn verdicts_survive_a_worktree_edit_a_head_move_and_a_repack() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let first = bare.commit_doc(DOC, None);
    let second = bare.commit_doc(b"# Architecture\n\nRewritten.\n", Some(&first));
    bare.git(&["update-ref", "refs/heads/main", &first], None);
    let claims = [
        claim("doc.md", DOC, &first),
        claim("doc.md", DOC, &second),
        claim("absent.md", DOC, &first),
    ];
    let before: Vec<_> = claims
        .iter()
        .map(|claim| verify(&bare.root, claim, &unbounded()))
        .collect();
    assert_eq!(
        before.iter().map(|e| e.verdict).collect::<Vec<_>>(),
        [
            Verdict::Match,
            Verdict::Mismatch,
            Verdict::Unavailable(Reason::PathAbsent)
        ]
    );

    // A linked worktree checks out the first commit; its file is then edited.
    let worktree = dir.path().join("worktree");
    bare.git(
        &[
            "worktree",
            "add",
            "--quiet",
            worktree.to_str().unwrap(),
            "main",
        ],
        None,
    );
    assert_eq!(std::fs::read(worktree.join("doc.md")).unwrap(), DOC);
    std::fs::write(worktree.join("doc.md"), b"edited in the worktree\n").unwrap();
    bare.git(&["update-ref", "refs/heads/main", &second], None);
    bare.git(&["repack", "-a", "-d", "--quiet"], None);
    assert!(!bare.loose_path(&first).exists());

    let after: Vec<_> = claims
        .iter()
        .map(|claim| verify(&bare.root, claim, &unbounded()))
        .collect();
    assert_eq!(after, before);
    // A ref name or an abbreviated id is no oid, so a HEAD move cannot change what it names.
    for oid in ["main", "HEAD", &first[..12]] {
        assert_eq!(
            verify(&bare.root, &claim("doc.md", DOC, oid), &unbounded()),
            unavailable(Reason::ObjectUnreadable(ObjectClass::Commit)),
            "{oid}"
        );
    }
}

#[test]
fn claim_paths_follow_the_document_path_rule() {
    let oid = "1111111111111111111111111111111111111111";
    let sixteen = vec!["d"; 16].join("/");
    assert!(Claim::new(&sixteen, [0; 32], 0, oid).is_ok());
    let seventeen = vec!["d"; 17].join("/");
    for path in [
        seventeen.as_str(),
        "",
        "/abs",
        "a//b",
        "./a",
        "a/../b",
        ".git/config",
    ] {
        assert_eq!(
            Claim::new(path, [0; 32], 0, oid),
            Err(InvalidClaimPath),
            "{path:?}"
        );
    }
}

#[test]
fn the_pure_verdict_compares_digests_after_the_header_gate() {
    let oid = "1111111111111111111111111111111111111111";
    let doc = claim("doc.md", b"one\r\ntwo\r\n", oid);
    let content = |bytes: &[u8]| Observed::Content {
        header_size: bytes.len() as u64,
        digests: Digests::of(bytes, doc.byte_length()),
    };
    assert_eq!(verdict(&doc, &content(b"one\r\ntwo\r\n")), Verdict::Match);
    assert_eq!(verdict(&doc, &content(b"one\ntwo\n")), Verdict::Match);
    assert_eq!(verdict(&doc, &content(b"one\ntwo!")), Verdict::Mismatch);
    assert_eq!(verdict(&doc, &content(b"one\r\ntwo\r!")), Verdict::Mismatch);
    // A raw digest match under a claimed length the header size does not equal is a mismatch.
    let wrong_length = Claim::new("doc.md", sha256(b"abc"), 4, oid).unwrap();
    let abc = Observed::Content {
        header_size: 3,
        digests: Digests::of(b"abc", 4),
    };
    assert_eq!(verdict(&wrong_length, &abc), Verdict::Mismatch);
    for reason in [
        Reason::PathAbsent,
        Reason::NotABlob(EntryClass::Gitlink),
        Reason::ObjectUnreadable(ObjectClass::Tree),
        Reason::OverBudget,
        Reason::Deadline(Stage::Header),
        Reason::RepositoryUnbound,
        Reason::RepositoryUnopenable,
    ] {
        assert_eq!(
            verdict(&doc, &Observed::Unavailable(reason)),
            Verdict::Unavailable(reason)
        );
    }
    // A header observed without content: the size decides, or no bytes were compared.
    let header = |header_size| verdict(&doc, &Observed::Header { header_size });
    assert_eq!(
        header(MAX_BLOB_BYTES + 1),
        Verdict::Unavailable(Reason::OverBudget)
    );
    assert_eq!(header(21), Verdict::Mismatch);
    assert_eq!(
        header(10),
        Verdict::Unavailable(Reason::ObjectUnreadable(ObjectClass::Blob))
    );
    // A header above the budget is over budget even when the digests agree.
    let matching = Digests {
        raw: *doc.digest(),
        normalized: None,
    };
    assert_eq!(
        verdict(
            &doc,
            &Observed::Content {
                header_size: MAX_BLOB_BYTES + 1,
                digests: matching,
            }
        ),
        Verdict::Unavailable(Reason::OverBudget)
    );
    assert_eq!(
        header_verdict(&doc, MAX_BLOB_BYTES + 1),
        Some(Verdict::Unavailable(Reason::OverBudget))
    );
    // The claimed 10 bytes are reachable from 5 to 20 header bytes.
    assert_eq!(header_verdict(&doc, 4), Some(Verdict::Mismatch));
    assert_eq!(header_verdict(&doc, 5), None);
    assert_eq!(header_verdict(&doc, 20), None);
    assert_eq!(header_verdict(&doc, 21), Some(Verdict::Mismatch));
    let empty = claim("doc.md", b"", oid);
    assert_eq!(header_verdict(&empty, 0), None);
    assert_eq!(header_verdict(&empty, 1), Some(Verdict::Mismatch));
    assert_eq!(header_verdict(&doc, 0), Some(Verdict::Mismatch));
}

#[test]
fn normalized_digests_exist_only_for_a_reachable_length() {
    let lf = b"a\nb\r\nc\n";
    assert_eq!(Digests::of(lf, 7).raw, sha256(lf));
    assert_eq!(Digests::of(lf, 7).normalized, None);
    assert_eq!(
        Digests::of(lf, 9).normalized,
        Some(sha256(b"a\r\nb\r\nc\r\n"))
    );
    assert_eq!(Digests::of(lf, 6).normalized, Some(sha256(b"a\nb\nc\n")));
    assert_eq!(Digests::of(lf, 8).normalized, None);
    assert_eq!(Digests::of(b"\n", 2).normalized, Some(sha256(b"\r\n")));
    // A lone CR is content, and an unterminated last segment keeps no ending.
    let lf_unterminated = b"a\rb\nc";
    let crlf_unterminated = b"a\rb\r\nc";
    assert_eq!(
        Digests::of(lf_unterminated, crlf_unterminated.len() as u64).normalized,
        Some(sha256(crlf_unterminated))
    );
    assert_eq!(
        Digests::of(crlf_unterminated, lf_unterminated.len() as u64).normalized,
        Some(sha256(lf_unterminated))
    );
}

#[test]
fn reasons_have_stable_names_and_a_terminal_split() {
    let names: Vec<_> = [
        Reason::PathAbsent,
        Reason::NotABlob(EntryClass::Symlink),
        Reason::ObjectUnreadable(ObjectClass::Blob),
        Reason::OverBudget,
        Reason::Deadline(Stage::Open),
        Reason::RepositoryUnbound,
        Reason::RepositoryUnopenable,
    ]
    .into_iter()
    .map(|reason| (reason.as_str(), reason.is_terminal()))
    .collect();
    assert_eq!(
        names,
        [
            ("path_absent", true),
            ("not_a_blob", true),
            ("object_unreadable", false),
            ("over_budget", true),
            ("deadline", false),
            ("repository_unbound", false),
            ("repository_unopenable", false),
        ]
    );
    assert_eq!(
        [EntryClass::Gitlink, EntryClass::Symlink, EntryClass::Tree].map(EntryClass::as_str),
        ["gitlink", "symlink", "tree"]
    );
    assert_eq!(
        [ObjectClass::Commit, ObjectClass::Tree, ObjectClass::Blob].map(ObjectClass::as_str),
        ["commit", "tree", "blob"]
    );
    assert_eq!(
        [
            Stage::Open,
            Stage::Commit,
            Stage::Tree,
            Stage::Header,
            Stage::Blob
        ]
        .map(Stage::as_str),
        ["open", "commit", "tree", "header", "blob"]
    );
}

#[test]
fn a_raw_digest_match_needs_the_claimed_length() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let commit = bare.commit_doc(b"abc", None);
    let wrong_length = Claim::new("doc.md", sha256(b"abc"), 4, &commit).unwrap();
    assert_eq!(
        verify(&bare.root, &wrong_length, &unbounded()),
        Evaluation {
            verdict: Verdict::Mismatch,
            bytes_read: 3,
        }
    );
    assert_eq!(
        verify(&bare.root, &claim("doc.md", b"abc", &commit), &unbounded()).verdict,
        Verdict::Match
    );
}

#[test]
fn an_empty_committed_file_matches_an_empty_claim() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let commit = bare.commit_doc(b"", None);
    let polls = PassesAt::new(None);
    assert_eq!(
        verify(&bare.root, &claim("doc.md", b"", &commit), &polls),
        Evaluation {
            verdict: Verdict::Match,
            bytes_read: 0,
        }
    );
    assert_eq!(polls.polls.borrow().last(), Some(&Stage::Blob));
    assert_eq!(
        verify(&bare.root, &claim("doc.md", b"x", &commit), &unbounded()),
        Evaluation {
            verdict: Verdict::Mismatch,
            bytes_read: 0,
        }
    );
}

#[test]
fn an_unsupported_object_format_is_unopenable() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    std::fs::write(
        bare.root.join("config"),
        "[core]\n\trepositoryformatversion = 1\n\tbare = true\n[extensions]\n\tobjectformat = sha256\n",
    )
    .unwrap();
    assert_eq!(
        verify(
            &bare.root,
            &claim("doc.md", DOC, &"0".repeat(64)),
            &unbounded()
        ),
        unavailable(Reason::RepositoryUnopenable)
    );
}

#[test]
fn a_blob_mode_entry_naming_another_kind_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let blob = bare.blob(DOC);
    let sub = bare.tree(&[("100644", "blob", &blob, "inner.md")]);
    let mut entry = b"100644 doc.md\0".to_vec();
    entry.extend_from_slice(gix::ObjectId::from_hex(sub.as_bytes()).unwrap().as_slice());
    let commit = bare.commit(&bare.literal("tree", &entry), None, "wrong blob kind");
    let sub_size = gix::open_opts(&bare.root, gix::open::Options::isolated())
        .unwrap()
        .find_header(gix::ObjectId::from_hex(sub.as_bytes()).unwrap())
        .unwrap()
        .size();
    // One length the subtree's size would decide as a mismatch, and one it would send to a read.
    for length in [sub_size * 3, sub_size] {
        let claim = Claim::new("doc.md", [0; 32], length, &commit).unwrap();
        assert_eq!(
            verify(&bare.root, &claim, &unbounded()),
            unavailable(Reason::ObjectUnreadable(ObjectClass::Blob)),
            "{length}"
        );
    }
}

#[test]
fn each_deadline_poll_precedes_its_read() {
    let dir = tempfile::tempdir().unwrap();
    for (index, (stage, class)) in [
        (Stage::Commit, ObjectClass::Commit),
        (Stage::Tree, ObjectClass::Tree),
        (Stage::Header, ObjectClass::Blob),
    ]
    .into_iter()
    .enumerate()
    {
        let bare = Bare::init(&dir.path().join(format!("repo-{index}.git")));
        let blob = bare.blob(DOC);
        let tree = bare.tree(&[("100644", "blob", &blob, "doc.md")]);
        let commit = bare.commit(&tree, None, "doc");
        let target = match class {
            ObjectClass::Commit => &commit,
            ObjectClass::Tree => &tree,
            ObjectClass::Blob => &blob,
        };
        bare.delete_loose(target);
        let doc = claim("doc.md", DOC, &commit);
        // A read before the poll would report the missing object instead of the deadline.
        assert_eq!(
            verify(&bare.root, &doc, &unbounded()),
            unavailable(Reason::ObjectUnreadable(class))
        );
        assert_eq!(
            verify(&bare.root, &doc, &PassesAt::new(Some(stage))),
            unavailable(Reason::Deadline(stage))
        );
    }
}

#[test]
fn a_non_bare_root_answers_from_the_object_store_not_the_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let repo = Bare { root: root.clone() };
    repo.git(&["init", "--quiet", "--initial-branch=main"], None);
    std::fs::write(root.join("doc.md"), DOC).unwrap();
    repo.git(&["add", "doc.md"], None);
    repo.git(&["commit", "--quiet", "-m", "doc"], None);
    let oid = repo.git(&["rev-parse", "HEAD"], None);
    let edited = b"edited in the worktree\n";
    std::fs::write(root.join("doc.md"), edited).unwrap();
    repo.git(&["add", "doc.md"], None);
    assert_eq!(
        verify(&root, &claim("doc.md", DOC, &oid), &unbounded()).verdict,
        Verdict::Match
    );
    assert_eq!(
        verify(&root, &claim("doc.md", edited, &oid), &unbounded()).verdict,
        Verdict::Mismatch
    );
}

/// The entry type codes of every object `bare`'s packs hold, read from each pack at its index's
/// offsets.
fn pack_entry_types(bare: &Bare) -> Vec<u8> {
    let pack_dir = bare.root.join("objects").join("pack");
    let mut types = Vec::new();
    for entry in std::fs::read_dir(&pack_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "idx") {
            continue;
        }
        let index = gix::odb::pack::index::File::at(&path, gix::hash::Kind::Sha1).unwrap();
        let pack = std::fs::read(path.with_extension("pack")).unwrap();
        for entry in index.iter() {
            let offset = usize::try_from(entry.pack_offset).unwrap();
            types.push((pack[offset] >> 4) & 0b111);
        }
    }
    types
}

#[test]
fn a_ref_delta_in_an_own_pack_decodes_against_its_base_in_that_pack() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let base: Vec<u8> = (0..200)
        .flat_map(|line| format!("line {line} of the architecture document\n").into_bytes())
        .collect();
    let mut edited = base.clone();
    edited.extend_from_slice(b"one more line\n");
    let first = bare.commit_doc(&base, None);
    let second = bare.commit_doc(&edited, Some(&first));
    bare.git(&["update-ref", "refs/heads/main", &second], None);
    bare.git(
        &["-c", "repack.useDeltaBaseOffset=false", "repack", "-adfq"],
        None,
    );
    bare.git(&["prune-packed"], None);
    // Type 7 is a ref-delta, whose base the entry names by object id.
    let types = pack_entry_types(&bare);
    assert!(types.contains(&7), "{types:?}");
    for (commit, bytes) in [(&first, &base), (&second, &edited)] {
        assert_eq!(
            verify(&bare.root, &claim("doc.md", bytes, commit), &unbounded()),
            Evaluation {
                verdict: Verdict::Match,
                bytes_read: bytes.len() as u64,
            }
        );
    }
}

#[test]
fn objects_in_an_alternate_with_more_packs_than_index_slots_match() {
    let dir = tempfile::tempdir().unwrap();
    let alternate = Bare::init(&dir.path().join("alternate.git"));
    let commit = alternate.commit_doc(DOC, None);
    let bare = fixture(dir.path());
    std::fs::write(
        bare.root.join("objects").join("info").join("alternates"),
        format!("{}\n", alternate.root.join("objects").display()),
    )
    .unwrap();
    let doc = claim("doc.md", DOC, &commit);
    let matched = Evaluation {
        verdict: Verdict::Match,
        bytes_read: DOC.len() as u64,
    };
    // The repository holds none of the claim's objects; its alternate holds them loose.
    assert_eq!(verify(&bare.root, &doc, &unbounded()), matched);

    // Forty-one packs in the alternate exceed the slots of the store the driver opens.
    let tree = alternate.git(&["rev-parse", &format!("{commit}^{{tree}}")], None);
    let blob = alternate.git(&["rev-parse", &format!("{commit}:doc.md")], None);
    let listing = format!("{commit}\n{tree}\n{blob}\n");
    alternate.git(
        &["pack-objects", "-q", "objects/pack/pack"],
        Some(listing.as_bytes()),
    );
    for index in 0..40 {
        let filler = alternate.blob(format!("filler {index}\n").as_bytes());
        alternate.git(
            &["pack-objects", "-q", "objects/pack/pack"],
            Some(format!("{filler}\n").as_bytes()),
        );
    }
    alternate.git(&["prune-packed"], None);
    let packs = std::fs::read_dir(alternate.root.join("objects").join("pack"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "idx")
        })
        .count();
    assert_eq!(packs, 41);
    assert!(!alternate.loose_path(&blob).exists());
    assert_eq!(verify(&bare.root, &doc, &unbounded()), matched);
}

#[test]
fn a_delta_with_an_empty_body_that_names_itself_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    for (index, (class, _, _)) in doc_objects().into_iter().enumerate() {
        let bare = Bare::init(&dir.path().join(format!("repo-{index}.git")));
        let mut commit = String::new();
        for (stored, kind, bytes) in doc_objects() {
            let id = if stored == class {
                let kind = gix::objs::Kind::from_bytes(kind.as_bytes()).unwrap();
                let id = gix::objs::compute_hash(gix::hash::Kind::Sha1, kind, &bytes).unwrap();
                support::git_repo::write_empty_self_delta_pack(&bare.root.join("objects"), id);
                id.to_string()
            } else {
                bare.literal(kind, &bytes)
            };
            if stored == ObjectClass::Commit {
                commit = id;
            }
        }
        assert_eq!(
            verify_within(&bare.root, &claim("doc.md", DOC, &commit)),
            unavailable(Reason::ObjectUnreadable(class)),
            "{class:?}"
        );
    }
}

#[test]
fn an_own_pack_blob_whose_stream_does_not_decode_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let blob = support::git_repo::write_truncated_blob_pack(&bare.root.join("objects"), DOC);
    let tree = bare.tree(&[("100644", "blob", &blob.to_string(), "doc.md")]);
    let commit = bare.commit(&tree, None, "truncated");
    let polls = PassesAt::new(None);
    assert_eq!(
        verify(&bare.root, &claim("doc.md", DOC, &commit), &polls),
        unavailable(Reason::ObjectUnreadable(ObjectClass::Blob))
    );
    // The header passed the gate, so the refusal came from the blob read.
    assert_eq!(polls.polls.borrow().last(), Some(&Stage::Blob));
}

#[test]
fn a_tree_above_the_allocation_limit_is_over_budget_loose_and_packed() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let blob = bare.blob(DOC);
    let mut listing = format!("100644 blob {blob}\tdoc.md\n");
    for index in 0..8000 {
        listing.push_str(&format!("100644 blob {blob}\tf{index:05}\n"));
    }
    let tree = bare.git(&["mktree"], Some(listing.as_bytes()));
    let size: u64 = bare.git(&["cat-file", "-s", &tree], None).parse().unwrap();
    assert!(size > MAX_BLOB_BYTES, "{size}");
    let commit = bare.commit(&tree, None, "wide");
    let doc = claim("doc.md", DOC, &commit);
    assert_eq!(
        verify(&bare.root, &doc, &unbounded()),
        unavailable(Reason::OverBudget)
    );

    bare.git(&["update-ref", "refs/heads/main", &commit], None);
    bare.git(&["repack", "-adfq"], None);
    bare.git(&["prune-packed"], None);
    assert!(!bare.loose_path(&tree).exists());
    assert_eq!(
        verify(&bare.root, &doc, &unbounded()),
        unavailable(Reason::OverBudget)
    );
}

#[test]
fn a_blob_only_in_an_alternate_with_more_packs_than_index_slots_matches() {
    let dir = tempfile::tempdir().unwrap();
    let alternate = Bare::init(&dir.path().join("alternate.git"));
    let blob = alternate.blob(DOC);
    let bare = fixture(dir.path());
    std::fs::write(
        bare.root.join("objects").join("info").join("alternates"),
        format!("{}\n", alternate.root.join("objects").display()),
    )
    .unwrap();
    let tree = bare.tree(&[("100644", "blob", &blob, "doc.md")]);
    let commit = bare.commit(&tree, None, "doc");
    alternate.git(
        &["pack-objects", "-q", "objects/pack/pack"],
        Some(format!("{blob}\n").as_bytes()),
    );
    for index in 0..40 {
        let filler = alternate.blob(format!("filler {index}\n").as_bytes());
        alternate.git(
            &["pack-objects", "-q", "objects/pack/pack"],
            Some(format!("{filler}\n").as_bytes()),
        );
    }
    alternate.git(&["prune-packed"], None);
    assert!(!alternate.loose_path(&blob).exists());
    assert!(bare.loose_path(&commit).exists() && bare.loose_path(&tree).exists());
    // The commit and tree read from the repository's loose store, so the blob's header is the
    // first lookup that reaches the object store.
    assert_eq!(
        verify(&bare.root, &claim("doc.md", DOC, &commit), &unbounded()),
        Evaluation {
            verdict: Verdict::Match,
            bytes_read: DOC.len() as u64,
        }
    );
}

#[test]
fn an_own_pack_delta_whose_base_lies_in_another_pack_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    for (index, (class, _, _)) in doc_objects().into_iter().enumerate() {
        let bare = Bare::init(&dir.path().join(format!("repo-{index}.git")));
        let base = gix::objs::compute_hash(
            gix::hash::Kind::Sha1,
            gix::objs::Kind::Blob,
            format!("base {index}\n").as_bytes(),
        )
        .unwrap();
        // The base's own pack holds it as a ref-delta on itself.
        support::git_repo::write_empty_self_delta_pack(&bare.root.join("objects"), base);
        let mut commit = String::new();
        for (stored, kind, bytes) in doc_objects() {
            let id = if stored == class {
                let kind = gix::objs::Kind::from_bytes(kind.as_bytes()).unwrap();
                let id = gix::objs::compute_hash(gix::hash::Kind::Sha1, kind, &bytes).unwrap();
                support::git_repo::write_ref_delta_pack(
                    &bare.root.join("objects"),
                    id,
                    base,
                    &bytes,
                );
                id.to_string()
            } else {
                bare.literal(kind, &bytes)
            };
            if stored == ObjectClass::Commit {
                commit = id;
            }
        }
        assert_eq!(
            verify_within(&bare.root, &claim("doc.md", DOC, &commit)),
            unavailable(Reason::ObjectUnreadable(class)),
            "{class:?}"
        );
    }
}

/// `verify` under an unbounded budget on its own thread; a call that runs past one minute fails
/// the test instead of stalling the run.
fn verify_within(root: &Path, claim: &Claim) -> Evaluation {
    let (root, claim) = (root.to_path_buf(), claim.clone());
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || sender.send(verify(&root, &claim, &unbounded())));
    receiver
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("verify returns within a minute")
}

#[test]
fn a_valid_loose_copy_answers_for_an_own_pack_entry_that_does_not_resolve() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let commit = bare.commit_doc(DOC, None);
    let blob = bare.git(&["rev-parse", &format!("{commit}:doc.md")], None);
    let blob = gix::ObjectId::from_hex(blob.as_bytes()).unwrap();
    let absent =
        gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, b"absent").unwrap();
    support::git_repo::write_ref_delta_pack(&bare.root.join("objects"), blob, absent, DOC);
    assert!(bare.loose_path(&blob.to_string()).exists());
    assert_eq!(
        verify_within(&bare.root, &claim("doc.md", DOC, &commit)),
        Evaluation {
            verdict: Verdict::Match,
            bytes_read: DOC.len() as u64,
        }
    );
}

#[test]
fn a_valid_own_pack_copy_answers_after_a_newer_pack_whose_chain_revisits_an_entry() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let commit = bare.commit_doc(DOC, None);
    let blob = bare.git(&["rev-parse", &format!("{commit}:doc.md")], None);
    let pack_dir = bare.root.join("objects").join("pack");
    bare.git(
        &["pack-objects", "-q", "objects/pack/pack"],
        Some(format!("{blob}\n").as_bytes()),
    );
    let valid: Vec<_> = std::fs::read_dir(&pack_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    let id = gix::ObjectId::from_hex(blob.as_bytes()).unwrap();
    support::git_repo::write_self_delta_pack(&bare.root.join("objects"), id, DOC);
    bare.git(&["prune-packed"], None);
    assert!(!bare.loose_path(&blob).exists());
    // The index of the self-referencing pack is the newest, so the walk reads it first.
    let now = std::time::SystemTime::now();
    for entry in std::fs::read_dir(&pack_dir).unwrap() {
        let path = entry.unwrap().path();
        let older = valid.contains(&path);
        let time = if older {
            now - std::time::Duration::from_secs(60)
        } else {
            now
        };
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(time)).unwrap();
    }
    assert_eq!(
        verify_within(&bare.root, &claim("doc.md", DOC, &commit)),
        Evaluation {
            verdict: Verdict::Match,
            bytes_read: DOC.len() as u64,
        }
    );
}

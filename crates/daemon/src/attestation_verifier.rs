//! The attestation verifier's core: a deterministic verdict for a claim that a commit's tree holds
//! named bytes at a path.
//!
//! A [`Claim`] names a relative path, the sha256 digest and byte length of a document's bytes, and
//! a commit oid. [`verify`] reads the repository's object store at that oid and returns exactly one
//! [`Verdict`]. [`Verdict::Match`] states that the bytes git stores for the path in the commit's
//! tree hash to the claimed digest, up to line-ending normalization; it states nothing about what
//! the document says or about any worktree. The git blob id addresses the bytes; the compared value
//! is the sha256 of the content.
//!
//! The work has two layers. [`verdict`] is a pure function of the claim and an [`Observed`] value,
//! with no repository handle and no clock. [`verify`] is the driver: it opens the repository
//! through the daemon's isolated opener with [`MAX_BLOB_BYTES`] as gix's allocation limit and
//! replacement refs ignored, finds the commit and walks its tree one path component at a time,
//! reads the blob's header before any content, reads at most one blob, and hands the observation
//! to [`verdict`]. The driver reads object-store contents only, so the verdict for one claim at one
//! oid is the same after any worktree edit, HEAD move, or repack.

use std::path::Path;

use gix::objs::tree::EntryKind;
use kernel::applicability::EvalBudget;
use sha2::{Digest as _, Sha256};

use crate::confined_path;
use crate::git_sources;

/// The per-blob budget: the document read cap source capture applies. The driver also installs it
/// as gix's allocation limit, so a commit, tree, blob, or delta base above it is refused before it
/// is allocated.
pub const MAX_BLOB_BYTES: u64 = 256 * 1024;

const ALLOCATION_LIMIT: std::num::NonZeroU64 = std::num::NonZeroU64::new(MAX_BLOB_BYTES).unwrap();

/// Bytes at a path in a commit's tree, named by their sha256 digest and length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    relative_path: String,
    digest: [u8; 32],
    byte_length: u64,
    oid: String,
}

/// The claim's path is outside the document path rule: `/`-separated ordinary components, at most
/// 16, none of them `.`, `..`, `.eidnara`, or `.git`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the claim's path is outside the document path rule")]
pub struct InvalidClaimPath;

impl Claim {
    /// A claim on `relative_path` at the commit `oid`, whose bytes have the sha256 `digest` and
    /// length `byte_length`. The path follows the rule document paths are published under, so the
    /// tree walk visits at most 16 components.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidClaimPath`] when the path is outside that rule.
    pub fn new(
        relative_path: &str,
        digest: [u8; 32],
        byte_length: u64,
        oid: &str,
    ) -> Result<Self, InvalidClaimPath> {
        confined_path::check(relative_path).map_err(|_| InvalidClaimPath)?;
        Ok(Self {
            relative_path: relative_path.to_owned(),
            digest,
            byte_length,
            oid: oid.to_owned(),
        })
    }

    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }

    pub fn byte_length(&self) -> u64 {
        self.byte_length
    }

    pub fn oid(&self) -> &str {
        &self.oid
    }
}

/// The outcome of checking one claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The bytes git stores for the path in the commit's tree hash to the claimed digest, up to
    /// line-ending normalization.
    Match,
    /// The blob at the path was read or its size decided the comparison, and its bytes differ from
    /// the claimed ones.
    Mismatch,
    /// The object store gave no bytes to compare.
    Unavailable(Reason),
}

/// Why a claim has no comparable bytes. Every reason is content-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The tree at the oid has no entry at the path.
    PathAbsent,
    /// The path names an entry whose content is not a blob in this repository, or lies beneath a
    /// gitlink or symlink.
    NotABlob(EntryClass),
    /// The object store could not return the object, or returned bytes that do not decode as that
    /// object or do not hash to its id. A missing object is unreadable, never absent.
    ObjectUnreadable(ObjectClass),
    /// The blob's header size is above [`MAX_BLOB_BYTES`], or materializing an object or a delta
    /// base needs an allocation above it.
    OverBudget,
    /// The deadline had passed at the poll before the named stage.
    Deadline(Stage),
    /// The claim's repository is bound to no project root.
    RepositoryUnbound,
    /// The repository could not be opened, or uses an object format this build does not read.
    RepositoryUnopenable,
}

/// The tree entry kinds that hold no blob content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryClass {
    Gitlink,
    Symlink,
    Tree,
}

/// The object kinds the driver reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectClass {
    Commit,
    Tree,
    Blob,
}

/// The driver's deadline polls, in order. `Open` precedes the repository open; `Tree` precedes
/// each tree read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Open,
    Commit,
    Tree,
    Header,
    Blob,
}

impl Reason {
    /// The stable snake_case name of the reason's class.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PathAbsent => "path_absent",
            Self::NotABlob(_) => "not_a_blob",
            Self::ObjectUnreadable(_) => "object_unreadable",
            Self::OverBudget => "over_budget",
            Self::Deadline(_) => "deadline",
            Self::RepositoryUnbound => "repository_unbound",
            Self::RepositoryUnopenable => "repository_unopenable",
        }
    }

    /// A terminal reason holds for every later read of the same claim at the same oid, because the
    /// commit's tree and the blob's size are fixed by the oid. The other reasons depend on the
    /// repository's current state or the clock.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::PathAbsent | Self::NotABlob(_) | Self::OverBudget
        )
    }
}

impl EntryClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gitlink => "gitlink",
            Self::Symlink => "symlink",
            Self::Tree => "tree",
        }
    }
}

impl ObjectClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Tree => "tree",
            Self::Blob => "blob",
        }
    }
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Commit => "commit",
            Self::Tree => "tree",
            Self::Header => "header",
            Self::Blob => "blob",
        }
    }
}

/// The digests of one blob's content: the raw bytes, and the one line-ending-normalized form whose
/// length equals the claimed length, when the content has one.
///
/// Normalization treats CRLF and LF as one line ending. A claimed length above the content's
/// selects the CRLF form, in which every LF without a preceding CR becomes CRLF: a blob stored with
/// LF whose worktree form has CRLF. A claimed length below it selects the LF form, in which every
/// CRLF becomes LF. A claimed length equal to the content's selects no normalized form, since a
/// conversion that changes a byte changes the length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Digests {
    pub raw: [u8; 32],
    pub normalized: Option<[u8; 32]>,
}

impl Digests {
    /// The digests of `content` for a claim of `claimed_length` bytes.
    pub fn of(content: &[u8], claimed_length: u64) -> Self {
        let raw = Sha256::digest(content).into();
        // Every line ending is the last byte of its segment, so a CRLF never spans two segments.
        let lines = || content.split_inclusive(|&byte| byte == b'\n');
        let crlf = lines().filter(|line| line.ends_with(b"\r\n")).count() as u64;
        let lone_lf = lines().filter(|line| line.ends_with(b"\n")).count() as u64 - crlf;
        let length = content.len() as u64;
        let (ending, other) = if claimed_length > length && length + lone_lf == claimed_length {
            (&b"\r\n"[..], &b"\n"[..])
        } else if claimed_length < length && length - crlf == claimed_length {
            (&b"\n"[..], &b"\r\n"[..])
        } else {
            return Self {
                raw,
                normalized: None,
            };
        };
        let mut hasher = Sha256::new();
        for line in lines() {
            match line.strip_suffix(other) {
                Some(text) if !(other == b"\n" && text.ends_with(b"\r")) => {
                    hasher.update(text);
                    hasher.update(ending);
                }
                _ => hasher.update(line),
            }
        }
        Self {
            raw,
            normalized: Some(hasher.finalize().into()),
        }
    }
}

/// What the driver observed for a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    /// A lookup or read ended before the blob's content was available.
    Unavailable(Reason),
    /// The blob's header size and the digests of its content.
    Content { header_size: u64, digests: Digests },
}

/// The verdict a blob's header size decides alone, before any content is read: a size above
/// [`MAX_BLOB_BYTES`] is over budget, and a size from which no line-ending form of the content can
/// reach the claimed length is a mismatch. A CRLF form is at most twice the content's length and an
/// LF form at least half of it.
pub fn header_verdict(claim: &Claim, header_size: u64) -> Option<Verdict> {
    if header_size > MAX_BLOB_BYTES {
        return Some(Verdict::Unavailable(Reason::OverBudget));
    }
    let claimed = claim.byte_length;
    let reachable =
        claimed <= header_size.saturating_mul(2) && claimed.saturating_mul(2) >= header_size;
    (!reachable).then_some(Verdict::Mismatch)
}

/// The verdict for `claim` given what the driver observed. `Match` when the raw digest or the
/// normalized digest equals the claimed digest.
pub fn verdict(claim: &Claim, observed: &Observed) -> Verdict {
    match observed {
        Observed::Unavailable(reason) => Verdict::Unavailable(*reason),
        Observed::Content {
            header_size,
            digests,
        } => header_verdict(claim, *header_size).unwrap_or(
            if digests.raw == claim.digest || digests.normalized == Some(claim.digest) {
                Verdict::Match
            } else {
                Verdict::Mismatch
            },
        ),
    }
}

/// A deadline the driver polls before opening the repository and before each object read. A read
/// that has started runs to completion under the allocation limit.
pub trait Deadline {
    /// Whether the deadline has passed at the poll before `stage`.
    fn passed(&self, stage: Stage) -> bool;
}

impl Deadline for EvalBudget {
    fn passed(&self, _stage: Stage) -> bool {
        self.is_exhausted()
    }
}

/// One claim's verdict and the blob content bytes the driver read for it: zero unless the blob was
/// read, and at most [`MAX_BLOB_BYTES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Evaluation {
    pub verdict: Verdict,
    pub bytes_read: u64,
}

/// Checks `claim` against the object store of the repository at `root` under `deadline`.
///
/// The repository is opened through the daemon's isolated opener with [`MAX_BLOB_BYTES`] as the
/// allocation limit. The driver reads the commit, one tree per path component, the blob's header,
/// and at most one blob, each looked up by object id; it reads no worktree file or index and
/// writes nothing.
pub fn verify(root: &Path, claim: &Claim, deadline: &impl Deadline) -> Evaluation {
    let mut bytes_read = 0;
    let observed = observe(root, claim, deadline, &mut bytes_read);
    Evaluation {
        verdict: observed.map_or_else(|decided| decided, |observed| verdict(claim, &observed)),
        bytes_read,
    }
}

fn unavailable(reason: Reason) -> Verdict {
    Verdict::Unavailable(reason)
}

fn poll(deadline: &impl Deadline, stage: Stage) -> Result<(), Verdict> {
    if deadline.passed(stage) {
        Err(unavailable(Reason::Deadline(stage)))
    } else {
        Ok(())
    }
}

/// Reads `id` as an object of `class`. An object that is missing, of another kind, or whose bytes
/// do not hash to `id` is unreadable; an allocation above the limit is over budget.
fn read_object<'repo>(
    repo: &'repo gix::Repository,
    id: gix::ObjectId,
    class: ObjectClass,
) -> Result<gix::Object<'repo>, Verdict> {
    let unreadable = unavailable(Reason::ObjectUnreadable(class));
    let object = repo.find_object(id).map_err(|error| match error {
        gix::object::find::existing::Error::Find(error)
            if git_sources::exceeds_allocation(&error) =>
        {
            unavailable(Reason::OverBudget)
        }
        _ => unreadable,
    })?;
    let kind = match class {
        ObjectClass::Commit => gix::object::Kind::Commit,
        ObjectClass::Tree => gix::object::Kind::Tree,
        ObjectClass::Blob => gix::object::Kind::Blob,
    };
    let hashes_to_id = gix::objs::compute_hash(repo.object_hash(), kind, &object.data)
        .is_ok_and(|actual| actual == id);
    if object.kind != kind || !hashes_to_id {
        return Err(unreadable);
    }
    Ok(object)
}

fn observe(
    root: &Path,
    claim: &Claim,
    deadline: &impl Deadline,
    bytes_read: &mut u64,
) -> Result<Observed, Verdict> {
    poll(deadline, Stage::Open)?;
    let repo = git_sources::open(root, ALLOCATION_LIMIT)
        .map_err(|_| unavailable(Reason::RepositoryUnopenable))?;
    let hash = repo.object_hash();
    git_sources::object_format(hash).map_err(|_| unavailable(Reason::RepositoryUnopenable))?;
    // An oid that does not parse for the repository's format names no commit the store can return.
    let commit_id = git_sources::parse_oid(&claim.oid, hash)
        .ok_or(unavailable(Reason::ObjectUnreadable(ObjectClass::Commit)))?;

    poll(deadline, Stage::Commit)?;
    let commit = read_object(&repo, commit_id, ObjectClass::Commit)?;
    let mut id = gix::objs::CommitRefIter::from_bytes(&commit.data, hash)
        .tree_id()
        .map_err(|_| unavailable(Reason::ObjectUnreadable(ObjectClass::Commit)))?;
    drop(commit);

    let mut components = claim.relative_path.split('/').peekable();
    while let Some(component) = components.next() {
        poll(deadline, Stage::Tree)?;
        let tree = read_object(&repo, id, ObjectClass::Tree)?;
        let mut found = None;
        for entry in gix::objs::TreeRefIter::from_bytes(&tree.data, hash) {
            let entry =
                entry.map_err(|_| unavailable(Reason::ObjectUnreadable(ObjectClass::Tree)))?;
            if entry.filename == component.as_bytes() {
                found = Some((entry.mode.kind(), entry.oid.to_owned()));
                break;
            }
        }
        let (kind, entry_id) = found.ok_or(unavailable(Reason::PathAbsent))?;
        let last = components.peek().is_none();
        match kind {
            EntryKind::Commit => return Err(unavailable(Reason::NotABlob(EntryClass::Gitlink))),
            EntryKind::Link => return Err(unavailable(Reason::NotABlob(EntryClass::Symlink))),
            EntryKind::Tree if last => {
                return Err(unavailable(Reason::NotABlob(EntryClass::Tree)));
            }
            EntryKind::Blob | EntryKind::BlobExecutable if !last => {
                return Err(unavailable(Reason::PathAbsent));
            }
            _ => id = entry_id,
        }
    }

    poll(deadline, Stage::Header)?;
    let unreadable_blob = unavailable(Reason::ObjectUnreadable(ObjectClass::Blob));
    let header = match repo.try_find_header(id) {
        Ok(Some(header)) if header.kind() == gix::object::Kind::Blob => header,
        _ => return Err(unreadable_blob),
    };
    let header_size = header.size();
    if let Some(verdict) = header_verdict(claim, header_size) {
        return Err(verdict);
    }

    poll(deadline, Stage::Blob)?;
    let blob = read_object(&repo, id, ObjectClass::Blob)?;
    *bytes_read = blob.data.len() as u64;
    if *bytes_read != header_size {
        return Err(unreadable_blob);
    }
    Ok(Observed::Content {
        header_size,
        digests: Digests::of(&blob.data, claim.byte_length),
    })
}

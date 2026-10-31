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
//! to [`verdict`], which decides every verdict. The driver reads object-store contents only, so the
//! verdict for one claim at one oid is the same after any worktree edit or HEAD move, and after a
//! repack except in one case: `over_budget` from the allocation limit for the commit, a tree on the
//! path, or the blob depends on how the pack stores that object, since a delta base, intermediate
//! result, or accumulated delta data can exceed the limit while the object itself fits, so a repack
//! can change it. The limit applies to every object read, so a commit or a tree above 256 KiB
//! (about 6,500 entries with short names) makes every claim beneath it `over_budget`.

use std::path::Path;

use gix::objs::tree::EntryKind;
use kernel::applicability::EvalBudget;
use ring::digest::{Context, SHA256};

use crate::confined_path;
use crate::git_sources;

/// The per-blob budget: the document read cap source capture applies. The driver also installs it
/// as gix's allocation limit, so a commit, tree, blob, or delta base above it is refused before it
/// is allocated. gix checks each object and base against the limit separately; its delta work
/// buffer for an admitted chain can reach about twice the limit plus the delta bytes.
pub const MAX_BLOB_BYTES: u64 = crate::project_docs::MAX_PROJECT_DOC_BYTES;

const ALLOCATION_LIMIT: std::num::NonZeroU64 = std::num::NonZeroU64::new(MAX_BLOB_BYTES).unwrap();

/// The pack index slots of the object store the driver opens. A store lookup that exhausts them
/// reopens the repository through `Objects::resize`.
const INDEX_SLOTS: u16 = 32;

/// Bytes at a path in a commit's tree, named by their sha256 digest and length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    relative_path: String,
    digest: [u8; 32],
    byte_length: u64,
    oid: String,
}

/// The claim's path is outside the confinement rule shared with document paths: at most 16
/// nonempty `/`-separated components, no NUL byte, and no `.`, `..`, or `.eidnara` or `.git` in any
/// ASCII case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the claim's path is outside the document path rule")]
pub struct InvalidClaimPath;

impl Claim {
    /// A claim on `relative_path` at the commit `oid`, whose bytes have the sha256 `digest` and
    /// length `byte_length`. The path satisfies the confinement rule document paths share, which
    /// bounds the tree walk to 16 components. The oid is parsed by [`verify`], where an oid
    /// that does not parse for the repository's object format is
    /// `Unavailable(object_unreadable(commit))`.
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
    /// The path's tree entry has tree, symlink, or gitlink mode, or the path lies beneath a symlink
    /// or gitlink entry. The entry mode decides before any read of the object it names.
    NotABlob(EntryClass),
    /// The requested object is missing, of the wrong kind, or fails an integrity check: its bytes
    /// do not fully decode as that object or do not hash to its id, its delta chain revisits an
    /// entry, or a blob's decoded length differs from its header size. A claim oid that does not
    /// parse for the repository's object format is `ObjectUnreadable(ObjectClass::Commit)`. A
    /// missing object is unreadable, never absent.
    ObjectUnreadable(ObjectClass),
    /// The blob's header size is above [`MAX_BLOB_BYTES`], or materializing the commit, a tree, or
    /// the blob needs an object or delta base above the allocation limit.
    OverBudget,
    /// The deadline had passed, or the budget was cancelled, at the poll before the named stage.
    Deadline(Stage),
    /// The claim's repository is bound to no project root.
    RepositoryUnbound,
    /// The repository could not be opened, or uses an object format this build does not read.
    RepositoryUnopenable,
}

/// The tree entry modes the driver does not read as document content.
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

    /// A terminal reason holds for every later read of the same claim at the same oid: the commit's
    /// tree fixes `path_absent` and `not_a_blob`, and the blob's size fixes `over_budget` from the
    /// header gate. `over_budget` from a delta chain above the allocation limit is also terminal
    /// here, though it depends on the pack layout and a repack can change it. The other reasons
    /// depend on the repository's current state or the clock.
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
/// Normalization compares the claim with one uniform conversion of the content. A claimed length
/// above the content's selects the CRLF form, in which every LF without a preceding CR becomes
/// CRLF: a blob stored with LF whose worktree form has CRLF. A claimed length below it selects the
/// LF form, in which every CRLF becomes LF. The converted length must equal the claimed length. A
/// claimed length equal to the content's selects no normalized form, since a conversion that
/// changes a byte changes the length. Claimed bytes whose line endings mix the two forms in any
/// other pattern match neither form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Digests {
    pub raw: [u8; 32],
    pub normalized: Option<[u8; 32]>,
}

impl Digests {
    /// The digests of `content` for a claim of `claimed_length` bytes.
    pub fn of(content: &[u8], claimed_length: u64) -> Self {
        let raw = sha256(content);
        let length = content.len() as u64;
        let no_form = Self {
            raw,
            normalized: None,
        };
        if claimed_length == length {
            return no_form;
        }
        // Every line ending is the last byte of its segment, so a CRLF never spans two segments.
        let lines = || content.split_inclusive(|&byte| byte == b'\n');
        let crlf = lines().filter(|line| line.ends_with(b"\r\n")).count() as u64;
        let lone_lf = lines().filter(|line| line.ends_with(b"\n")).count() as u64 - crlf;
        let ending: &[u8] = if claimed_length > length && length + lone_lf == claimed_length {
            b"\r\n"
        } else if claimed_length < length && length - crlf == claimed_length {
            b"\n"
        } else {
            return no_form;
        };
        // Each line's ending, with or without its CR, becomes the selected ending; an unterminated
        // last segment is kept as is.
        let mut hasher = Context::new(&SHA256);
        for line in lines() {
            match line.strip_suffix(b"\n") {
                Some(text) => {
                    hasher.update(text.strip_suffix(b"\r").unwrap_or(text));
                    hasher.update(ending);
                }
                None => hasher.update(line),
            }
        }
        Self {
            raw,
            normalized: Some(finish(hasher)),
        }
    }
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Context::new(&SHA256);
    hasher.update(bytes);
    finish(hasher)
}

fn finish(hasher: Context) -> [u8; 32] {
    let mut out = [0; 32];
    out.copy_from_slice(hasher.finish().as_ref());
    out
}

/// What the driver observed for a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    /// A lookup, poll, or read ended before the blob's content was compared.
    Unavailable(Reason),
    /// The blob's header size, read without its content because the size decides the verdict.
    Header { header_size: u64 },
    /// The blob's header size and the digests of its content, computed for the claim's
    /// `byte_length`.
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

/// The verdict for `claim` given what the driver observed. The header size decides first; then
/// `Match` when the header size equals the claimed length and the raw digest equals the claimed
/// digest, or when the normalized digest, whose length equals the claimed length, equals it. A
/// header size
/// that decides nothing leaves no bytes to compare, so a header observed without content is
/// `Unavailable(object_unreadable(blob))`.
pub fn verdict(claim: &Claim, observed: &Observed) -> Verdict {
    match observed {
        Observed::Unavailable(reason) => Verdict::Unavailable(*reason),
        Observed::Header { header_size } => header_verdict(claim, *header_size).unwrap_or(
            Verdict::Unavailable(Reason::ObjectUnreadable(ObjectClass::Blob)),
        ),
        Observed::Content {
            header_size,
            digests,
        } => header_verdict(claim, *header_size).unwrap_or(
            if (*header_size == claim.byte_length && digests.raw == claim.digest)
                || digests.normalized == Some(claim.digest)
            {
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
/// read, and at most [`MAX_BLOB_BYTES`]. Commit and tree bytes are not counted. A blob whose
/// decoded bytes then fail the kind, hash, or header-length check counts its decoded length.
/// `blob_oid` is the hex id of the blob entry the tree walk reached at the path, set from that
/// entry onward, so every `Match` and `Mismatch` carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub verdict: Verdict,
    pub bytes_read: u64,
    pub blob_oid: Option<String>,
}

/// Checks `claim` against the object store of the repository at `root` under `deadline`.
///
/// The repository is opened through the daemon's isolated opener with [`MAX_BLOB_BYTES`] as the
/// allocation limit. The driver reads the commit, one tree per path component, the blob's header,
/// and at most one blob, each looked up by object id; it reads no worktree file or index and
/// writes nothing.
pub fn verify(root: &Path, claim: &Claim, deadline: &impl Deadline) -> Evaluation {
    let mut bytes_read = 0;
    let mut blob_oid = None;
    let observed = lookup(root, claim, deadline, &mut bytes_read, &mut blob_oid)
        .unwrap_or_else(Observed::Unavailable);
    Evaluation {
        verdict: verdict(claim, &observed),
        bytes_read,
        blob_oid: blob_oid.map(|id| id.to_string()),
    }
}

fn poll(deadline: &impl Deadline, stage: Stage) -> Result<(), Reason> {
    if deadline.passed(stage) {
        Err(Reason::Deadline(stage))
    } else {
        Ok(())
    }
}

struct Objects<'repo> {
    root: &'repo Path,
    repo: &'repo gix::Repository,
    reopened: bool,
    resized: Option<gix::Repository>,
    packs: git_sources::OwnPacks,
    loose: gix::odb::loose::Store,
    data: Vec<u8>,
}

impl<'repo> Objects<'repo> {
    fn new(root: &'repo Path, repo: &'repo gix::Repository) -> Self {
        let options = gix::odb::loose::Options {
            alloc_limit_bytes: usize::try_from(MAX_BLOB_BYTES).ok(),
            ..Default::default()
        };
        Self {
            root,
            repo,
            reopened: false,
            resized: None,
            packs: git_sources::OwnPacks::new(repo, ALLOCATION_LIMIT),
            loose: gix::odb::loose::Store::at_opts(
                repo.objects.store_ref().path(),
                repo.object_hash(),
                options,
            ),
            data: Vec::new(),
        }
    }

    /// `Objects::resize` reopens the repository after an object-store lookup exhausts its index
    /// slots.
    fn resize(&mut self) -> bool {
        if self.reopened {
            return false;
        }
        self.reopened = true;
        self.resized = git_sources::open(self.root, ALLOCATION_LIMIT).ok();
        self.resized.is_some()
    }

    fn store(&self) -> &gix::Repository {
        self.resized.as_ref().unwrap_or(self.repo)
    }

    fn store_header(&mut self, id: gix::ObjectId) -> Option<(gix::object::Kind, u64)> {
        let mut found = self.store().try_find_header(id);
        if matches!(&found, Err(error) if git_sources::exhausts_slots(&error.0)) && self.resize() {
            found = self.store().try_find_header(id);
        }
        found
            .ok()
            .flatten()
            .map(|header| (header.kind(), header.size()))
    }

    fn store_find(
        &mut self,
        id: gix::ObjectId,
    ) -> Result<gix::object::Kind, gix::object::find::existing::Error> {
        let object = self.resized.as_ref().unwrap_or(self.repo).find_object(id)?;
        self.data.clear();
        self.data.extend_from_slice(&object.data);
        Ok(object.kind)
    }

    fn blob_header(&mut self, id: gix::ObjectId) -> Result<u64, Reason> {
        let unreadable = Reason::ObjectUnreadable(ObjectClass::Blob);
        let packed = self.packs.header(&id);
        let header = match packed {
            Ok(Some(header)) => Some((header.kind, header.object_size)),
            packed => match self.loose.try_header(&id) {
                Ok(Some((size, kind))) => Some((kind, size)),
                Ok(None) if packed.is_ok() => self.store_header(id),
                Ok(None) | Err(_) => None,
            },
        };
        match header {
            Some((gix::object::Kind::Blob, size)) => Ok(size),
            _ => Err(unreadable),
        }
    }

    fn read(
        &mut self,
        id: gix::ObjectId,
        class: ObjectClass,
        decoded: &mut u64,
    ) -> Result<&[u8], Reason> {
        use git_sources::OwnPackError;
        use gix::object::find::existing::Error as StoreFind;
        use gix::odb::pack::data::decode::Error as PackDecode;
        let unreadable = Reason::ObjectUnreadable(class);
        let packed = self.packs.decode(&id, &mut self.data);
        let found = match packed {
            Ok(Some(kind)) => kind,
            Err(OwnPackError::Decode(PackDecode::OutOfMemory)) => return Err(Reason::OverBudget),
            packed => match self.loose.try_find(&id, &mut self.data) {
                Ok(Some(object)) => object.kind,
                Err(gix::odb::loose::find::Error::OutOfMemory { .. }) => {
                    return Err(Reason::OverBudget);
                }
                Ok(None) if packed.is_ok() => {
                    let mut found = self.store_find(id);
                    if matches!(&found, Err(StoreFind::Find(error)) if git_sources::exhausts_slots(error))
                        && self.resize()
                    {
                        found = self.store_find(id);
                    }
                    found.map_err(|error| match error {
                        StoreFind::Find(error) if git_sources::exceeds_allocation(&error) => {
                            Reason::OverBudget
                        }
                        _ => unreadable,
                    })?
                }
                Ok(None) | Err(_) => return Err(unreadable),
            },
        };
        *decoded = decoded.saturating_add(self.data.len() as u64);
        let kind = match class {
            ObjectClass::Commit => gix::object::Kind::Commit,
            ObjectClass::Tree => gix::object::Kind::Tree,
            ObjectClass::Blob => gix::object::Kind::Blob,
        };
        let hashes_to_id = gix::objs::compute_hash(self.repo.object_hash(), kind, &self.data)
            .is_ok_and(|actual| actual == id);
        if found != kind || !hashes_to_id {
            return Err(unreadable);
        }
        Ok(&self.data)
    }
}

/// What the object store holds for `claim`, setting `bytes_read` to the blob content bytes decoded
/// and `blob_oid` to the blob entry the path resolves to.
fn lookup(
    root: &Path,
    claim: &Claim,
    deadline: &impl Deadline,
    bytes_read: &mut u64,
    blob_oid: &mut Option<gix::ObjectId>,
) -> Result<Observed, Reason> {
    poll(deadline, Stage::Open)?;
    let slots = gix::odb::store::init::Slots::Given(INDEX_SLOTS);
    let repo = git_sources::open_with_slots(root, ALLOCATION_LIMIT, slots)
        .map_err(|_| Reason::RepositoryUnopenable)?;
    let hash = repo.object_hash();
    git_sources::object_format(hash).map_err(|_| Reason::RepositoryUnopenable)?;
    // An oid that does not parse for the repository's format names no commit the store can return.
    let commit_id = git_sources::parse_oid(&claim.oid, hash)
        .ok_or(Reason::ObjectUnreadable(ObjectClass::Commit))?;

    let mut objects = Objects::new(root, &repo);

    poll(deadline, Stage::Commit)?;
    let commit = objects.read(commit_id, ObjectClass::Commit, &mut 0)?;
    // The whole commit and each whole tree decode, so an object with a valid prefix and a malformed
    // remainder is unreadable.
    let mut id = gix::objs::CommitRef::from_bytes(commit, hash)
        .map_err(|_| Reason::ObjectUnreadable(ObjectClass::Commit))?
        .tree();

    let mut components = claim.relative_path.split('/').peekable();
    while let Some(component) = components.next() {
        poll(deadline, Stage::Tree)?;
        let tree = objects.read(id, ObjectClass::Tree, &mut 0)?;
        let mut found = None;
        for entry in gix::objs::TreeRefIter::from_bytes(tree, hash) {
            let entry = entry.map_err(|_| Reason::ObjectUnreadable(ObjectClass::Tree))?;
            if found.is_none() && entry.filename == component.as_bytes() {
                found = Some((entry.mode.kind(), entry.oid.to_owned()));
            }
        }
        let (kind, entry_id) = found.ok_or(Reason::PathAbsent)?;
        let last = components.peek().is_none();
        match (kind, last) {
            (EntryKind::Tree, false) | (EntryKind::Blob | EntryKind::BlobExecutable, true) => {
                id = entry_id;
            }
            (EntryKind::Tree, true) => return Err(Reason::NotABlob(EntryClass::Tree)),
            (EntryKind::Blob | EntryKind::BlobExecutable, false) => {
                return Err(Reason::PathAbsent);
            }
            (EntryKind::Link, true | false) => return Err(Reason::NotABlob(EntryClass::Symlink)),
            (EntryKind::Commit, true | false) => {
                return Err(Reason::NotABlob(EntryClass::Gitlink));
            }
        }
    }

    *blob_oid = Some(id);
    poll(deadline, Stage::Header)?;
    let unreadable_blob = Reason::ObjectUnreadable(ObjectClass::Blob);
    let header_size = objects.blob_header(id)?;
    if header_verdict(claim, header_size).is_some() {
        return Ok(Observed::Header { header_size });
    }

    poll(deadline, Stage::Blob)?;
    let blob = objects.read(id, ObjectClass::Blob, bytes_read)?;
    if *bytes_read != header_size {
        return Err(unreadable_blob);
    }
    Ok(Observed::Content {
        header_size,
        digests: Digests::of(blob, claim.byte_length),
    })
}

pub mod attest;
pub mod classify;

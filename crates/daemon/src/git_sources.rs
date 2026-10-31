//! Reads an explicit, bounded selection of commits from one bound repository and turns each commit's exact message bytes into a `git_commits` source unit keyed by repository id, object format, and full object id. Nothing is traversed: a commit not named in the selection is not read, and a name, path, or ref is never an identity.
//!
//! Object headers, the store's allocations, and decoded sizes are bounded independently; a selection is refused whole if any object is missing, corrupt, non-commit, repeated, or exceeds a bound. A non-UTF-8 message or a declared non-UTF-8 encoding is an explicit disposition rather than a converted or dropped row.

use std::cell::OnceCell;
use std::collections::HashSet;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use gix::bstr::ByteSlice;
use gix::odb::store::init::Slots;
use kernel::source_identity::OccurrenceClass;

use crate::harness_sources::{Representation, SourceUnit};
use crate::projection_gates::{Denial, EntryPoint, HookGate, ProjectionHook};

/// A commit's bytes name the repository and object they were read from. The store treats that as affirmative provenance: an asserted `Normal` class is kept instead of folded to `Sensitive`, so the publisher's `sensitivity` decides whether repository text may leave the host. `None` for a unit that is not a commit or lacks either field; the encoder has already refused the latter.
pub(crate) fn commit_provenance(unit: &SourceUnit) -> Option<kernel::RepositoryProvenance> {
    if unit.class != OccurrenceClass::GitCommits {
        return None;
    }
    Some(kernel::RepositoryProvenance {
        repository_id: unit.identity_value("repository_id")?.to_owned(),
        revision: unit.identity_value("oid")?.to_owned(),
    })
}

/// The occurrence revision of every commit: a commit object never changes under its id, so its only revision is the first.
pub const GIT_COMMIT_REVISION: &str = "1";

/// The role a commit unit carries; provenance, never identity or text.
pub const COMMIT_ROLE: &str = "commit";

/// Bytes of a declared encoding name a disposition retains, escaped to ASCII.
pub const MAX_DECLARED_ENCODING_BYTES: usize = 32;

/// One repository the daemon reads commits from. `repository_id` is the stable identity every occurrence names; `path` is only where the objects are read from and is not an identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryBinding {
    pub repository_id: String,
    pub path: PathBuf,
}

/// Charges one selection is judged against, from the object headers before any object is decoded and again from the decoded bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitReadBounds {
    pub max_commits: NonZeroUsize,
    /// Decoded object bytes one commit may occupy; a larger object can never carry an admissible message.
    pub max_object_bytes: NonZeroU64,
    /// Decoded object bytes the whole selection may occupy.
    pub max_total_object_bytes: NonZeroU64,
}

/// Identifies why a selection produces no units.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitRefusal {
    #[error("the selection names {count} commits, above the bound of {max}")]
    SelectionTooLarge { count: usize, max: usize },
    #[error("the selection names {0} twice")]
    DuplicateOid(String),
    #[error("the repository could not be opened")]
    Open,
    #[error("the repository's object format is not supported")]
    UnsupportedObjectFormat,
    /// The repository has a shallow boundary, so a traversal from its refs cannot reach every ancestor the object store holds.
    #[error("the repository is shallow")]
    Shallow,
    /// Stores the invalid entry's zero-based selection index rather than its unvalidated bytes.
    #[error("selection entry {0} is not a full lowercase object id of the repository's format")]
    MalformedOid(usize),
    #[error("object {0} is missing")]
    MissingObject(String),
    #[error("object {0} is not a commit")]
    NotACommit(String),
    /// The object store failed to read or decode the object; absence is not inferred from it.
    #[error("object {0} could not be read")]
    Unreadable(String),
    /// The returned bytes do not hash to the id, so they cannot represent that object.
    #[error("object {0} does not hash to its id")]
    HashMismatch(String),
    #[error("object {oid} decodes to {bytes} bytes, above the bound of {max}")]
    ObjectTooLarge { oid: String, bytes: u64, max: u64 },
    /// Materializing the object needs an allocation above the per-object bound. The store refuses before those bytes exist, so no decoded size is known.
    #[error("object {oid} needs more than {max} bytes to materialize")]
    DecodeTooLarge { oid: String, max: u64 },
    #[error("the selection decodes to {bytes} bytes, above the bound of {max}")]
    TotalBytesExceeded { bytes: u64, max: u64 },
    /// The gate denied git ingest or its durable rows; the repository was not opened.
    #[error("the projection gate denied the git source hooks: {0}")]
    Denied(Denial),
}

/// Why one selected commit yields no unit although the selection was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitDisposition {
    /// The message declares an encoding other than UTF-8, or its bytes are not UTF-8; the bytes are retained nowhere and never converted. `declared` is the header's value escaped to ASCII and cut at [`MAX_DECLARED_ENCODING_BYTES`].
    UnsupportedEncoding {
        oid: String,
        declared: Option<String>,
    },
}

/// The units of one selection in selection order and the commits the selection excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSelection {
    pub object_format: &'static str,
    pub units: Vec<SourceUnit>,
    pub dispositions: Vec<GitDisposition>,
}

/// `gix::hash::Kind` is non-exhaustive, so the wildcard refuses a hash kind this build does not read.
pub(crate) fn object_format(hash: gix::hash::Kind) -> Result<&'static str, GitRefusal> {
    match hash {
        gix::hash::Kind::Sha1 => Ok("sha1"),
        _ => Err(GitRefusal::UnsupportedObjectFormat),
    }
}

/// gix reports an `extensions.objectFormat` value this build does not read as a typed-string error on that key rather than a dedicated variant, so the refusal is recognized by the key it names.
pub(crate) fn declares_unsupported_object_format(error: &gix::open::Error) -> bool {
    match error {
        gix::open::Error::Config(gix::config::Error::ConfigTypedString(error)) => {
            error.key == "extensions.objectFormat"
        }
        _ => false,
    }
}

/// Repository options without user, system, or installation configuration, with gix's allocation limit set to `max_object_bytes` after the repository's own configuration.
pub(crate) fn isolated_options(max_object_bytes: NonZeroU64) -> gix::open::Options {
    gix::open::Options::isolated().config_overrides([format!(
        "gitoxide.objects.allocLimit={}",
        max_object_bytes.get()
    )])
}

pub(crate) fn open(
    path: &Path,
    max_object_bytes: NonZeroU64,
) -> Result<gix::Repository, GitRefusal> {
    open_with_slots(path, max_object_bytes, Slots::default())
}

/// Opens the repository without user, system, or installation configuration and with replacement refs ignored, so neither a repository's own settings nor a `refs/replace` entry can substitute another object for a selected id.
///
/// `max_object_bytes` is also installed as gix's allocation limit. A packed delta's header states only the final object size, while materializing it inflates every base in its chain; the limit makes the store refuse a base above the bound instead of allocating it. The override is applied after the repository's own configuration, so a repository cannot raise it.
pub(crate) fn open_with_slots(
    path: &Path,
    max_object_bytes: NonZeroU64,
    slots: Slots,
) -> Result<gix::Repository, GitRefusal> {
    let options = isolated_options(max_object_bytes).object_store_slots(slots);
    let mut repo = gix::open_opts(path, options).map_err(|error| {
        if declares_unsupported_object_format(&error) {
            GitRefusal::UnsupportedObjectFormat
        } else {
            GitRefusal::Open
        }
    })?;
    // gix's default slot sizing resolves the alternates chain and lists every
    // database's `pack` directory while the store opens (gix-odb
    // `store_impls/dynamic/init.rs`). Pack-directory listing errors other than
    // `NotFound` cause gix's default store opening to fail; this check applies
    // the same refusals to a given slot count.
    if matches!(slots, Slots::Given(_)) && !sizing_inputs_readable(repo.objects.store_ref().path())
    {
        return Err(GitRefusal::Open);
    }
    repo.objects.ignore_replacements = true;
    Ok(repo)
}

fn sizing_inputs_readable(objects: &Path) -> bool {
    let mut dirs = if matches!(
        objects.join("info").join("alternates").try_exists(),
        Ok(false)
    ) {
        Vec::new()
    } else {
        let Ok(cwd) = std::env::current_dir() else {
            return false;
        };
        let Ok(alternates) = gix::odb::alternate::resolve(objects.to_owned(), &cwd) else {
            return false;
        };
        alternates
    };
    dirs.insert(0, objects.to_owned());
    dirs.iter().all(|dir| {
        std::fs::read_dir(dir.join("pack")).map_or_else(
            |error| error.kind() == std::io::ErrorKind::NotFound,
            |_| true,
        )
    })
}

/// The lookup returns `id`'s kind by decoding headers from `repo`'s own loose objects, then its pack files. Loose read errors and unusable pack lookups count as misses.
///
/// The pack read tries `hint`'s index first when it lies in this repository's `objects/pack`, then the other indices newest first, and stores the index that answers in `hint`. Both the hinted and the scanned lookups require regular index and pack files.
///
/// # Errors
///
/// A pack whose index describes another pack, or whose delta chain revisits an entry, produces [`PackCorruption`].
pub(crate) fn file_object_kind(
    repo: &gix::Repository,
    id: &gix::oid,
    hint: &Mutex<Option<PathBuf>>,
) -> Result<Option<gix::object::Kind>, PackCorruption> {
    let objects = repo.objects.store_ref().path();
    let hash = repo.object_hash();
    let loose = gix::odb::loose::Store::at(objects, hash)
        .try_header(id)
        .ok()
        .flatten()
        .map(|(_, kind)| kind);
    match loose {
        Some(kind) => Ok(Some(kind)),
        None => packed_kind(&objects.join("pack"), hash, id, hint),
    }
}

fn packed_kind(
    pack_dir: &Path,
    hash: gix::hash::Kind,
    id: &gix::oid,
    hint: &Mutex<Option<PathBuf>>,
) -> Result<Option<gix::object::Kind>, PackCorruption> {
    let hinted = hint
        .lock()
        .ok()
        .and_then(|hint| hint.clone())
        .filter(|path| path.parent() == Some(pack_dir));
    if let Some(path) = hinted.as_deref()
        && let Some(kind) = index_kind(path, hash, id)?
    {
        return Ok(Some(kind));
    }
    for path in indices_newest_first(pack_dir, hinted.as_deref()) {
        if let Some(kind) = index_kind(&path, hash, id)? {
            if let Ok(mut hint) = hint.lock() {
                *hint = Some(path);
            }
            return Ok(Some(kind));
        }
    }
    Ok(None)
}

/// The `.idx` files in `pack_dir` other than `skip`, newest first; an unreadable directory lists none.
fn indices_newest_first(pack_dir: &Path, skip: Option<&Path>) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(pack_dir) else {
        return Vec::new();
    };
    let mut indices: Vec<_> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            let is_index = entry.file_type().is_ok_and(|kind| kind.is_file())
                && path.extension().is_some_and(|extension| extension == "idx");
            if !is_index || skip == Some(path.as_path()) {
                return None;
            }
            Some((entry.metadata().ok()?.modified().ok()?, path))
        })
        .collect();
    indices.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    indices.into_iter().map(|(_, path)| path).collect()
}

/// The packs in a repository's own `objects/pack`. `OwnPacks` lists the packs once and caches each
/// opened index and pack for delta-chain checks and decodes. A decode refuses an allocation above
/// `alloc_limit`. `entry_header` bounds delta-chain hops by `index.num_objects()` before
/// `decode_header` reads the delta body.
pub(crate) struct OwnPacks {
    dir: PathBuf,
    hash: gix::hash::Kind,
    alloc_limit: usize,
    listed: Option<Vec<OwnPack>>,
    last: Option<usize>,
}

struct OwnPack {
    path: PathBuf,
    index: OnceCell<Option<gix::odb::pack::index::File>>,
    data: OnceCell<Option<gix::odb::pack::data::File>>,
}

#[derive(Debug)]
pub(crate) enum OwnPackError {
    Corruption,
    Decode(gix::odb::pack::data::decode::Error),
}

impl OwnPacks {
    pub(crate) fn new(repo: &gix::Repository, alloc_limit: NonZeroU64) -> Self {
        Self {
            dir: repo.objects.store_ref().path().join("pack"),
            hash: repo.object_hash(),
            alloc_limit: usize::try_from(alloc_limit.get()).unwrap_or(usize::MAX),
            listed: None,
            last: None,
        }
    }

    /// The header of `id`'s entry in the first own pack whose index holds it and whose entry
    /// decodes, trying the index that answered last, then the others newest first. `None` when no
    /// opened own index contains `id`.
    ///
    /// # Errors
    ///
    /// `header` returns [`OwnPackError::Corruption`] after exhausting the search if an opened index
    /// contains `id` and every matching pack is unavailable, fails `entry_header`, or yields `None`.
    pub(crate) fn header(
        &mut self,
        id: &gix::oid,
    ) -> Result<Option<gix::odb::pack::data::decode::header::Outcome>, OwnPackError> {
        Ok(self.locate(id)?.map(|(_, _, header)| header))
    }

    /// `OwnPacks::decode` decodes `id` into `out` from the pack selected by [`Self::header`],
    /// resolves ref-delta bases through that pack's index, and returns the object's kind; `None`
    /// when [`Self::header`] is `None`.
    ///
    /// # Errors
    ///
    /// `OwnPacks::decode` propagates [`Self::header`]'s errors and returns
    /// [`OwnPackError::Decode`] when the entry does not decode within the allocation limit.
    pub(crate) fn decode(
        &mut self,
        id: &gix::oid,
        out: &mut Vec<u8>,
    ) -> Result<Option<gix::object::Kind>, OwnPackError> {
        use gix::odb::pack::data::decode::entry::ResolvedBase;
        let Some((index, data, _)) = self.locate(id)? else {
            return Ok(None);
        };
        let entry_at = |id: &gix::oid| {
            let position = index.lookup(id)?;
            data.entry(index.pack_offset_at_index(position)).ok()
        };
        let entry = entry_at(id).ok_or(OwnPackError::Corruption)?;
        let resolve = |base: &gix::oid, _: &mut Vec<u8>| entry_at(base).map(ResolvedBase::InPack);
        data.decode_entry(
            entry,
            out,
            &mut gix::zlib::Inflate::default(),
            &resolve,
            &mut gix::odb::pack::cache::Never,
        )
        .map(|outcome| Some(outcome.kind))
        .map_err(OwnPackError::Decode)
    }

    fn locate(&mut self, id: &gix::oid) -> Result<Option<Located<'_>>, OwnPackError> {
        let (hash, alloc_limit) = (self.hash, self.alloc_limit);
        let listed = self.listed.get_or_insert_with(|| {
            indices_newest_first(&self.dir, None)
                .into_iter()
                .map(|path| OwnPack {
                    path,
                    index: OnceCell::new(),
                    data: OnceCell::new(),
                })
                .collect()
        });
        let last = self.last;
        let order = last
            .into_iter()
            .chain((0..listed.len()).filter(|&position| Some(position) != last));
        let mut held = false;
        for position in order {
            let pack = &listed[position];
            let index = pack.index.get_or_init(|| open_index(&pack.path, hash));
            let Some(index) = index.as_ref().filter(|index| index.lookup(id).is_some()) else {
                continue;
            };
            held = true;
            let data = pack.data.get_or_init(|| {
                gix::odb::pack::data::File::at(pack.path.with_extension("pack"), hash)
                    .ok()
                    .map(|data| data.with_alloc_limit_bytes(Some(alloc_limit)))
            });
            let Some(data) = data else {
                continue;
            };
            if let Ok(Some(header)) = entry_header(index, data, id) {
                self.last = Some(position);
                return Ok(Some((index, data, header)));
            }
        }
        if held {
            Err(OwnPackError::Corruption)
        } else {
            Ok(None)
        }
    }
}

type Located<'packs> = (
    &'packs gix::odb::pack::index::File,
    &'packs gix::odb::pack::data::File,
    gix::odb::pack::data::decode::header::Outcome,
);

/// A pack of the repository holds the looked-up object in a form whose header cannot be decoded.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PackCorruption {
    /// The delta chain revisits one of the pack's entries.
    CyclicDeltaChain,
    /// The index's pack checksum differs from the pack file's trailer, so the index's offsets describe another pack.
    MismatchedIndex,
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

/// Reads `id`'s kind from the pack `path` indexes.
fn index_kind(
    path: &Path,
    hash: gix::hash::Kind,
    id: &gix::oid,
) -> Result<Option<gix::object::Kind>, PackCorruption> {
    let Some(index) = open_index(path, hash) else {
        return Ok(None);
    };
    if index.lookup(id).is_none() {
        return Ok(None);
    }
    let Ok(data) = gix::odb::pack::data::File::at(path.with_extension("pack"), hash) else {
        return Ok(None);
    };
    Ok(entry_header(&index, &data, id)?.map(|header| header.kind))
}

/// The index at `path` when it and its pack are regular files and the index reader accepts it.
fn open_index(path: &Path, hash: gix::hash::Kind) -> Option<gix::odb::pack::index::File> {
    if !is_regular_file(path) || !is_regular_file(&path.with_extension("pack")) {
        return None;
    }
    gix::odb::pack::index::File::at(path, hash).ok()
}

fn entry_header(
    index: &gix::odb::pack::index::File,
    data: &gix::odb::pack::data::File,
    id: &gix::oid,
) -> Result<Option<gix::odb::pack::data::decode::header::Outcome>, PackCorruption> {
    use gix::odb::pack::data::{decode::header::ResolvedBase, entry::Header};
    let Some(position) = index.lookup(id) else {
        return Ok(None);
    };
    if index.pack_checksum() != data.checksum() {
        return Err(PackCorruption::MismatchedIndex);
    }
    let entry_at = |position| data.entry(index.pack_offset_at_index(position)).ok();
    let Some(first) = entry_at(position) else {
        return Ok(None);
    };
    let mut entry = first.clone();
    let mut hops: u32 = 0;
    loop {
        let base = match entry.header {
            Header::OfsDelta { base_distance } => entry
                .checked_base_pack_offset(base_distance)
                .and_then(|offset| data.entry(offset).ok()),
            Header::RefDelta { base_id } => index.lookup(base_id).and_then(entry_at),
            Header::Commit | Header::Tree | Header::Blob | Header::Tag => break,
        };
        let Some(base) = base else {
            return Ok(None);
        };
        hops += 1;
        if hops > index.num_objects() {
            return Err(PackCorruption::CyclicDeltaChain);
        }
        entry = base;
    }
    let in_pack = |base: &gix::oid| entry_at(index.lookup(base)?).map(ResolvedBase::InPack);
    Ok(data
        .decode_header(first, &mut gix::zlib::Inflate::default(), &in_pack)
        .ok())
}

/// The object store's lookup needed more pack index slots than [`open_with_slots`] received.
pub(crate) struct SlotsExhausted;

/// The kind of object `id` from `repo`'s object store. Store errors read as absent objects.
pub(crate) fn store_object_kind(
    repo: &gix::Repository,
    id: &gix::oid,
) -> Result<Option<gix::object::Kind>, SlotsExhausted> {
    match repo.try_find_header(id) {
        Ok(header) => Ok(header.map(|header| header.kind())),
        Err(error) if exhausts_slots(&error.0) => Err(SlotsExhausted),
        Err(_) => Ok(None),
    }
}

/// Whether the store ran out of pack index slots while looking up the object or any base in its delta chain. The store wraps a base lookup's failure in `DeltaBaseLookup`.
pub(crate) fn exhausts_slots(error: &gix::objs::find::Error) -> bool {
    use gix::odb::store::{find::Error as Store, load_index::Error as LoadIndex};
    fn walk(error: &Store) -> bool {
        match error {
            Store::LoadIndex(LoadIndex::InsufficientSlots { .. }) => true,
            Store::DeltaBaseLookup { err, .. } => walk(err),
            _ => false,
        }
    }
    error.downcast_ref::<Store>().is_some_and(walk)
}

/// Whether the store refused an object because materializing it needs an allocation above its limit, anywhere in a delta chain.
pub(crate) fn exceeds_allocation(error: &gix::objs::find::Error) -> bool {
    use gix::odb::store::find::Error as Store;
    fn walk(error: &Store) -> bool {
        match error {
            Store::Pack(gix::odb::pack::data::decode::Error::OutOfMemory)
            | Store::Loose(gix::odb::loose::find::Error::OutOfMemory { .. }) => true,
            Store::DeltaBaseLookup { err, .. } => walk(err),
            _ => false,
        }
    }
    error.downcast_ref::<Store>().is_some_and(walk)
}

/// Returns `None` unless `oid` is a full lowercase hexadecimal id for `hash`.
pub(crate) fn parse_oid(oid: &str, hash: gix::hash::Kind) -> Option<gix::ObjectId> {
    if oid.len() != hash.len_in_hex()
        || !oid
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    gix::ObjectId::from_hex(oid.as_bytes()).ok()
}

fn escaped_prefix(bytes: &[u8]) -> String {
    bytes[..bytes.len().min(MAX_DECLARED_ENCODING_BYTES)]
        .escape_ascii()
        .to_string()
}

/// Reads the commits `oids` name from `binding` under `bounds`.
///
/// # Errors
///
/// Returns [`GitRefusal`] when the selection is over its bound or names an id twice, the repository cannot be opened or uses an unsupported format, an id is malformed, an object is missing, unreadable, or not a commit, or the object sizes exceed the bounds before, during, or after decoding. No unit is produced from a refused selection.
pub fn read_selection(
    gate: &HookGate,
    binding: &RepositoryBinding,
    oids: &[String],
    bounds: GitReadBounds,
) -> Result<GitSelection, GitRefusal> {
    gate.admit_all(
        &[ProjectionHook::GitIngest, ProjectionHook::GitDurableRows],
        EntryPoint::Dispatch,
    )
    .map_err(GitRefusal::Denied)?;
    if oids.len() > bounds.max_commits.get() {
        return Err(GitRefusal::SelectionTooLarge {
            count: oids.len(),
            max: bounds.max_commits.get(),
        });
    }
    let repo = open(&binding.path, bounds.max_object_bytes)?;
    let hash = repo.object_hash();
    let format = object_format(hash)?;
    let mut seen = HashSet::new();
    let ids: Vec<gix::ObjectId> = oids
        .iter()
        .enumerate()
        .map(|(index, oid)| {
            let id = parse_oid(oid, hash).ok_or(GitRefusal::MalformedOid(index))?;
            if !seen.insert(id) {
                return Err(GitRefusal::DuplicateOid(oid.clone()));
            }
            Ok(id)
        })
        .collect::<Result<_, _>>()?;
    let charge = |oid: &str, size: u64, total: &mut u64| -> Result<(), GitRefusal> {
        if size > bounds.max_object_bytes.get() {
            return Err(GitRefusal::ObjectTooLarge {
                oid: oid.to_owned(),
                bytes: size,
                max: bounds.max_object_bytes.get(),
            });
        }
        *total = total.saturating_add(size);
        if *total > bounds.max_total_object_bytes.get() {
            return Err(GitRefusal::TotalBytesExceeded {
                bytes: *total,
                max: bounds.max_total_object_bytes.get(),
            });
        }
        Ok(())
    };
    // Every header is judged before any object is decoded, so an oversized or missing object refuses the selection before a byte of message is read. A header's size is the store's claim about the decoded object; the decode pass charges the bytes it actually produced.
    let mut total = 0u64;
    for (oid, id) in oids.iter().zip(&ids) {
        let header = match repo.try_find_header(*id) {
            Ok(Some(header)) => header,
            Ok(None) => return Err(GitRefusal::MissingObject(oid.clone())),
            Err(_) => return Err(GitRefusal::Unreadable(oid.clone())),
        };
        if header.kind() != gix::object::Kind::Commit {
            return Err(GitRefusal::NotACommit(oid.clone()));
        }
        charge(oid, header.size(), &mut total)?;
    }
    let mut units = Vec::with_capacity(ids.len());
    let mut dispositions = Vec::new();
    let mut decoded_total = 0u64;
    for (oid, id) in oids.iter().zip(&ids) {
        let object = repo.find_object(*id).map_err(|error| match error {
            gix::object::find::existing::Error::NotFound { .. } => {
                GitRefusal::MissingObject(oid.clone())
            }
            gix::object::find::existing::Error::Find(error) if exceeds_allocation(&error) => {
                GitRefusal::DecodeTooLarge {
                    oid: oid.clone(),
                    max: bounds.max_object_bytes.get(),
                }
            }
            _ => GitRefusal::Unreadable(oid.clone()),
        })?;
        charge(oid, object.data.len() as u64, &mut decoded_total)?;
        // A hash mismatch refuses the selection rather than retaining bytes under an id that does not identify them.
        let actual = gix::objs::compute_hash(hash, object.kind, &object.data)
            .map_err(|_| GitRefusal::Unreadable(oid.clone()))?;
        if actual != *id {
            return Err(GitRefusal::HashMismatch(oid.clone()));
        }
        let commit = object
            .try_into_commit()
            .map_err(|_| GitRefusal::NotACommit(oid.clone()))?;
        let decoded = commit
            .decode()
            .map_err(|_| GitRefusal::Unreadable(oid.clone()))?;
        // gix recognizes `encoding` only directly after `committer`; git honors it at any header position, and JGit writes it after `gpgsig`. Either position declares the message's encoding.
        let declared = decoded
            .encoding
            .or_else(|| decoded.extra_headers().find("encoding"));
        let utf8_declared = declared.is_none_or(|encoding| {
            encoding.eq_ignore_ascii_case(b"utf-8") || encoding.eq_ignore_ascii_case(b"utf8")
        });
        let text = if utf8_declared {
            decoded.message.to_str().ok()
        } else {
            None
        };
        let Some(text) = text else {
            dispositions.push(GitDisposition::UnsupportedEncoding {
                oid: oid.clone(),
                declared: declared.map(|encoding| escaped_prefix(encoding)),
            });
            continue;
        };
        units.push(SourceUnit {
            class: OccurrenceClass::GitCommits,
            identity: vec![
                ("repository_id", binding.repository_id.clone()),
                ("object_format", format.to_owned()),
                ("oid", oid.clone()),
            ],
            revision: GIT_COMMIT_REVISION.to_owned(),
            representation: Representation::CommitMessage,
            text: text.to_owned(),
            role: COMMIT_ROLE.to_owned(),
            outcome: None,
        });
    }
    Ok(GitSelection {
        object_format: format,
        units,
        dispositions,
    })
}

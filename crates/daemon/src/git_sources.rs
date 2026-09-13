//! Reads an explicit, bounded selection of commits from one bound repository and turns each commit's exact message bytes into a `git_commits` source unit keyed by repository id, object format, and full object id. Nothing is traversed: a commit not named in the selection is not read, and a name, path, or ref is never an identity.
//!
//! Object headers, the store's allocations, and decoded sizes are bounded independently; a selection is refused whole if any object is missing, corrupt, non-commit, repeated, or exceeds a bound. A non-UTF-8 message or a declared non-UTF-8 encoding is an explicit disposition rather than a converted or dropped row.

use std::collections::HashSet;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;
use kernel::source_identity::OccurrenceClass;

use crate::harness_sources::{Representation, SourceUnit};
use crate::projection_gates::{Denial, EntryPoint, HookGate, ProjectionHook};

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
fn declares_unsupported_object_format(error: &gix::open::Error) -> bool {
    match error {
        gix::open::Error::Config(gix::config::Error::ConfigTypedString(error)) => {
            error.key == "extensions.objectFormat"
        }
        _ => false,
    }
}

/// Opens the repository without user, system, or installation configuration and with replacement refs ignored, so neither a repository's own settings nor a `refs/replace` entry can substitute another object for a selected id.
///
/// `max_object_bytes` is also installed as gix's allocation limit. A packed delta's header states only the final object size, while materializing it inflates every base in its chain; the limit makes the store refuse a base above the bound instead of allocating it. The override is applied after the repository's own configuration, so a repository cannot raise it.
pub(crate) fn open(
    path: &Path,
    max_object_bytes: NonZeroU64,
) -> Result<gix::Repository, GitRefusal> {
    let options = gix::open::Options::isolated().config_overrides([format!(
        "gitoxide.objects.allocLimit={}",
        max_object_bytes.get()
    )]);
    let mut repo = gix::open_opts(path, options).map_err(|error| {
        if declares_unsupported_object_format(&error) {
            GitRefusal::UnsupportedObjectFormat
        } else {
            GitRefusal::Open
        }
    })?;
    repo.objects.ignore_replacements = true;
    Ok(repo)
}

/// Whether the store refused an object because materializing it needs an allocation above its limit, anywhere in a delta chain.
fn exceeds_allocation(error: &gix::objs::find::Error) -> bool {
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
fn parse_oid(oid: &str, hash: gix::hash::Kind) -> Option<gix::ObjectId> {
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
        });
    }
    Ok(GitSelection {
        object_format: format,
        units,
        dispositions,
    })
}

//! Bounded cheap checks against a checkout snapshot's worktree.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, OnceLock};

use sha2::{Digest, Sha256};

use super::checkout::{CheckoutSnapshot, EvalBudget, WorktreeEntry};
use super::payloads::CheckSpec;

/// Maximum config bytes read for one cheap check.
pub const MAX_CONFIG_BYTES: u64 = 1 << 20;

/// Maximum config bytes one batch retains across every check path. Each path
/// is read once per batch and its content held for the rest of it. Without
/// this cap, a checkout can control resident memory.
pub const MAX_CHECK_CACHE_BYTES: u64 = 16 * MAX_CONFIG_BYTES;

/// The evaluator maps `Unsupported` to uncertain rather than pass or fail.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    Passed,
    Failed { evidence: String },
    Unsupported { evidence: String },
    BudgetExhausted,
}

/// Absence is settled by the shape probe before a read is attempted, so a read
/// either yields content or leaves the key unevaluated: a permission error, a
/// transient I/O error, a vanished path, or a file past the size cap.
#[derive(Debug, Clone)]
enum ConfigRead {
    Content(Arc<ConfigContent>),
    Unevaluated(String),
}

impl ConfigRead {
    /// Digest material naming the content this batch read, so a cache key can
    /// distinguish two runs that read different bytes at one path.
    fn observation(&self) -> String {
        match self {
            Self::Content(content) => content.observation.clone(),
            Self::Unevaluated(reason) => format!("unevaluated:{reason}"),
        }
    }
}

/// One config file's bytes with the derived values every check against it
/// shares: the digest is fixed at read time and each parse runs at most
/// once, so K checks on one path cost one hash and one parse per batch.
#[derive(Debug)]
struct ConfigContent {
    text: String,
    observation: String,
    json: OnceLock<Option<serde_json::Value>>,
    yaml: OnceLock<Option<Vec<serde_norway::Value>>>,
}

impl ConfigContent {
    fn new(text: String) -> Self {
        let mut hash = Sha256::new();
        hash.update(text.as_bytes());
        Self {
            text,
            observation: format!("content:{:x}", hash.finalize()),
            json: OnceLock::new(),
            yaml: OnceLock::new(),
        }
    }

    /// `None` when the document is not JSON.
    fn json(&self) -> Option<&serde_json::Value> {
        self.json
            .get_or_init(|| serde_json::from_str(&self.text).ok())
            .as_ref()
    }

    /// `None` unless every document in the YAML stream parses and at least one
    /// is a mapping or sequence. A TOML or INI file parses as one plain scalar
    /// or fails, so structure here means the file is YAML; the line heuristic
    /// applies otherwise. commentlint: allow(JUDGE)
    fn yaml(&self) -> Option<&[serde_norway::Value]> {
        self.yaml
            .get_or_init(|| {
                use serde::Deserialize;
                let documents = serde_norway::Deserializer::from_str(&self.text)
                    .map(serde_norway::Value::deserialize)
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;
                documents
                    .iter()
                    .any(|value| {
                        matches!(
                            value,
                            serde_norway::Value::Mapping(_) | serde_norway::Value::Sequence(_)
                        )
                    })
                    .then_some(documents)
            })
            .as_deref()
    }
}

/// Whether a declared path is a file a check can read, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Resolved {
    /// Present and a regular file, established without following a symlink.
    RegularFile,
    /// Resolved beneath the worktree and definitely not there.
    Absent,
    /// Present, and established to be a shape no check can read: a directory,
    /// socket, device, or FIFO. No regular file is at this path, which is as
    /// definite an answer about the declared file as absence — unlike a
    /// symlink, whose unfollowed target could be one.
    NotAFile(String),
    /// Nothing was read: the spelling leaves the worktree, or an inspection
    /// failed, or a symlink was refused rather than followed.
    Unresolvable(String),
}

impl Resolved {
    fn observation(&self) -> &str {
        match self {
            Self::RegularFile => "regular-file",
            Self::Absent => "absent",
            Self::NotAFile(reason) | Self::Unresolvable(reason) => reason,
        }
    }
}

/// Single authority on what one batch observed in the worktree.
///
/// The object cache key and the check that produces a verdict both read
/// through here, so a cached verdict can never describe bytes its key does
/// not. Reading the live filesystem twice would let a path change between the
/// two reads and revert afterwards, storing a verdict under a key no later
/// request reproduces.
#[derive(Debug, Default)]
pub struct CheckCache {
    resolved: HashMap<String, Resolved>,
    contents: HashMap<String, ConfigRead>,
    /// Total content bytes held in `contents`; never exceeds
    /// `MAX_CHECK_CACHE_BYTES`.
    retained_bytes: u64,
}

impl CheckCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn resolve(&mut self, snapshot: &CheckoutSnapshot, path: &str) -> Resolved {
        if let Some(cached) = self.resolved.get(path) {
            return cached.clone();
        }
        let resolved = match snapshot.worktree_entry(path) {
            WorktreeEntry::RegularFile => Resolved::RegularFile,
            WorktreeEntry::Absent => Resolved::Absent,
            // A terminal symlink is where the target could sit outside the
            // checkout, so no check follows one to a verdict.
            WorktreeEntry::Symlink => Resolved::Unresolvable(format!(
                "check path {path} is a symlink, whose target this check will not follow"
            )),
            WorktreeEntry::Directory => Resolved::NotAFile(format!(
                "check path {path} is a directory, not a file a check can read"
            )),
            WorktreeEntry::Other => {
                Resolved::NotAFile(format!("check path {path} is not a regular file"))
            }
            WorktreeEntry::Unresolvable(reason) => Resolved::Unresolvable(reason),
        };
        self.resolved.insert(path.to_string(), resolved.clone());
        resolved
    }

    /// The exhausted-budget outcome is cached like any other, so the cache
    /// key and the verdict describe the same single read.
    fn read(&mut self, snapshot: &CheckoutSnapshot, path: &str) -> ConfigRead {
        if let Some(cached) = self.contents.get(path) {
            return cached.clone();
        }
        let outcome = match read_bounded(snapshot, path) {
            ConfigRead::Content(content)
                if self.retained_bytes + content.text.len() as u64 > MAX_CHECK_CACHE_BYTES =>
            {
                ConfigRead::Unevaluated("config read budget exhausted".to_string())
            }
            ConfigRead::Content(content) => {
                self.retained_bytes += content.text.len() as u64;
                ConfigRead::Content(content)
            }
            unevaluated => unevaluated,
        };
        self.contents.insert(path.to_string(), outcome.clone());
        outcome
    }
}

/// Digest material for the worktree state `check` reads, or `None` for a check
/// whose verdict does not depend on the filesystem.
pub(super) fn check_observation(
    cache: &mut CheckCache,
    snapshot: &CheckoutSnapshot,
    check: &CheckSpec,
) -> Option<String> {
    match check {
        CheckSpec::FileExists { path } => {
            Some(cache.resolve(snapshot, path).observation().to_string())
        }
        CheckSpec::ConfigKey { path, .. } => {
            let resolved = cache.resolve(snapshot, path);
            let shape = resolved.observation().to_string();
            // Only a regular file is read, so only then is there content to
            // name; the shape alone settles the other cases.
            match resolved {
                Resolved::RegularFile => {
                    let content = cache.read(snapshot, path).observation();
                    Some(format!("{shape}\u{1f}{content}"))
                }
                Resolved::Absent | Resolved::NotAFile(_) | Resolved::Unresolvable(_) => Some(shape),
            }
        }
        // Both are `Unsupported` whatever the worktree holds.
        CheckSpec::Symbol { .. } | CheckSpec::Unrecognized => None,
    }
}

/// Reads through a descriptor walk that follows no symlink at any level, and
/// sizes the file by that descriptor.
///
/// A pathname re-resolved after a containment check can escape: a concurrent
/// checkout that replaces an ancestor directory with a symlink redirects every
/// later pathname operation, and `NOFOLLOW` guards only the final component.
/// FIFOs can block reads and character devices such as `/dev/zero` produce
/// valid UTF-8 indefinitely, both of which the open refuses.
fn read_bounded(snapshot: &CheckoutSnapshot, path: &str) -> ConfigRead {
    let file = match snapshot.open_worktree_regular(path) {
        Ok(Some(file)) => file,
        // Resolution already saw a regular file here, so the path moved.
        Ok(None) => {
            return ConfigRead::Unevaluated("path is no longer a regular file".to_string());
        }
        Err(error) => return ConfigRead::Unevaluated(error.to_string()),
    };
    match file.metadata() {
        Ok(metadata) if metadata.len() > MAX_CONFIG_BYTES => {
            return ConfigRead::Unevaluated(format!("file exceeds {MAX_CONFIG_BYTES} bytes"));
        }
        Ok(_) => {}
        Err(error) => return ConfigRead::Unevaluated(error.to_string()),
    }
    let mut content = String::new();
    // Metadata size can race file growth and does not always bound the stream,
    // so `MAX_CONFIG_BYTES` is enforced on bytes read.
    match file.take(MAX_CONFIG_BYTES + 1).read_to_string(&mut content) {
        Ok(_) if content.len() as u64 > MAX_CONFIG_BYTES => {
            ConfigRead::Unevaluated(format!("file exceeds {MAX_CONFIG_BYTES} bytes"))
        }
        Ok(_) => ConfigRead::Content(Arc::new(ConfigContent::new(content))),
        Err(error) => ConfigRead::Unevaluated(error.to_string()),
    }
}

/// `Unsupported` rather than `Failed`: the check was never evaluated, so the
/// checked object is uncertain rather than definitely stale.
fn unevaluated(evidence: String) -> CheckOutcome {
    CheckOutcome::Unsupported { evidence }
}

/// Runs one check natively against the snapshot's worktree. File existence
/// and config-key presence ship here; symbol resolution returns
/// `Unsupported` until a real resolver exists.
pub fn run_cheap_check(
    snapshot: &CheckoutSnapshot,
    check: &CheckSpec,
    budget: &EvalBudget,
    cache: &mut CheckCache,
) -> CheckOutcome {
    if budget.is_exhausted() {
        return CheckOutcome::BudgetExhausted;
    }
    match check {
        CheckSpec::FileExists { path } => match cache.resolve(snapshot, path) {
            Resolved::RegularFile => CheckOutcome::Passed,
            Resolved::Absent => CheckOutcome::Failed {
                evidence: format!("file {path} does not exist in the checkout"),
            },
            // No regular file is here, which the check asked about; repair can
            // act on that exactly as it acts on an absent path.
            Resolved::NotAFile(reason) => CheckOutcome::Failed { evidence: reason },
            // A path that was never read is not a definite absence.
            Resolved::Unresolvable(reason) => unevaluated(reason),
        },
        CheckSpec::ConfigKey { path, key } => {
            match cache.resolve(snapshot, path) {
                Resolved::RegularFile => {}
                Resolved::Absent => {
                    return CheckOutcome::Failed {
                        evidence: format!("config file {path} does not exist in the checkout"),
                    };
                }
                // `Failed` here would claim the file exists and omits the key,
                // which is what repair would then try to edit. A shape that is
                // not a config file leaves the key unevaluated instead.
                Resolved::NotAFile(reason) | Resolved::Unresolvable(reason) => {
                    return unevaluated(reason);
                }
            }
            let content = match cache.read(snapshot, path) {
                ConfigRead::Content(content) => content,
                // The key may well be defined; the read never got to look.
                ConfigRead::Unevaluated(reason) => {
                    return unevaluated(format!("config file {path} could not be read: {reason}"));
                }
            };
            match config_contains_key(&content, key) {
                KeyPresence::Present => CheckOutcome::Passed,
                KeyPresence::Absent => CheckOutcome::Failed {
                    evidence: format!("config file {path} does not define key {key}"),
                },
                KeyPresence::Undecidable(reason) => {
                    unevaluated(format!("config file {path}: {reason}"))
                }
            }
        }
        CheckSpec::Symbol { path, symbol } => CheckOutcome::Unsupported {
            evidence: format!("symbol check for {symbol} in {path} is not supported yet"),
        },
        CheckSpec::Unrecognized => CheckOutcome::Unsupported {
            evidence: "check kind is not recognized".to_string(),
        },
    }
}

/// JSON carries no line structure, so a minified document has to be parsed
/// rather than scanned; the line heuristic below reports every key in
/// `{"flag":true}` missing.
fn json_contains_key(value: &serde_json::Value, key: &str) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            // Any depth, matching what the line scan finds in a pretty-printed
            // document.
            map.contains_key(key) || map.values().any(|value| json_contains_key(value, key))
        }
        serde_json::Value::Array(items) => items.iter().any(|item| json_contains_key(item, key)),
        _ => false,
    }
}

/// A line scan cannot tell a mapping key from the same text inside a block
/// scalar (`description: |` followed by an indented `enabled: true`), so a
/// parsed YAML document is walked structurally like JSON. Only string keys
/// are compared. commentlint: allow(JUDGE)
fn yaml_contains_key(value: &serde_norway::Value, key: &str) -> bool {
    match value {
        serde_norway::Value::Mapping(map) => map.iter().any(|(name, nested)| {
            matches!(name, serde_norway::Value::String(name) if name == key)
                || yaml_contains_key(nested, key)
        }),
        serde_norway::Value::Sequence(items) => {
            items.iter().any(|item| yaml_contains_key(item, key))
        }
        serde_norway::Value::Tagged(tagged) => yaml_contains_key(&tagged.value, key),
        _ => false,
    }
}

/// What a config document says about one key.
enum KeyPresence {
    Present,
    Absent,
    /// The document has a shape the line heuristic cannot read safely.
    Undecidable(String),
}

/// Structured documents (JSON, YAML) are walked; the remaining line-oriented
/// formats (TOML, INI) use a presence heuristic: the key must open a line
/// (after whitespace and optional quoting) and be followed by a delimiter.
/// A TOML multi-line string can hold a line shaped exactly like an
/// assignment, so a document containing one is undecidable rather than
/// scanned. commentlint: allow(JUDGE)
fn config_contains_key(content: &ConfigContent, key: &str) -> KeyPresence {
    if let Some(value) = content.json() {
        return present(json_contains_key(value, key));
    }
    if let Some(documents) = content.yaml() {
        return present(
            documents
                .iter()
                .any(|document| yaml_contains_key(document, key)),
        );
    }
    if content.text.contains("\"\"\"") || content.text.contains("'''") {
        return KeyPresence::Undecidable(
            "multi-line strings make key presence undecidable by line scan".to_string(),
        );
    }
    present(content.text.lines().any(|line| {
        let line = line.trim_start();
        let line = line.strip_prefix(['"', '\'']).unwrap_or(line);
        let Some(rest) = line.strip_prefix(key) else {
            return false;
        };
        let rest = rest.strip_prefix(['"', '\'']).unwrap_or(rest);
        let rest = rest.trim_start();
        rest.starts_with('=') || rest.starts_with(':')
    }))
}

fn present(found: bool) -> KeyPresence {
    if found {
        KeyPresence::Present
    } else {
        KeyPresence::Absent
    }
}

/// Whether the worktree state a check observed for `path` is still the state
/// the index records, so the snapshot's clean dirty gate still describes it.
///
/// The snapshot's dirty gate and a check's live read are two observations of
/// one path; an edit between them pairs a clean gate with content the gate
/// never saw. A tracked path is compared by blob id against its index entry;
/// an untracked path that is present and not ignored appeared after the
/// snapshot. `None` when the path is tracked but the check read no content,
/// which leaves nothing to compare. commentlint: allow(JUDGE)
pub(super) fn observation_matches_index(
    cache: &mut CheckCache,
    snapshot: &CheckoutSnapshot,
    path: &str,
) -> Option<bool> {
    let repo = snapshot.repo();
    let index = repo.index_or_empty().ok()?;
    let entry = index.entry_by_path(path.into());
    match cache.resolve(snapshot, path) {
        Resolved::RegularFile => {}
        // A tracked path that is no longer a regular file diverged from the
        // index; an untracked one cannot be told from an ignored one here.
        Resolved::Absent | Resolved::NotAFile(_) => return Some(entry.is_none()),
        Resolved::Unresolvable(_) => return None,
    }
    let Some(entry) = entry else {
        // Present and untracked: only an ignored path is consistent with the
        // clean gate the snapshot took.
        let mut excludes = repo
            .excludes(
                &index,
                None,
                gix::worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
            )
            .ok()?;
        let platform = excludes
            .at_entry(path, Some(gix::index::entry::Mode::FILE))
            .ok()?;
        return Some(platform.is_excluded());
    };
    let ConfigRead::Content(content) = cache.read(snapshot, path) else {
        return None;
    };
    let blob = gix::objs::compute_hash(
        repo.object_hash(),
        gix::objs::Kind::Blob,
        content.text.as_bytes(),
    )
    .ok()?;
    Some(blob == entry.id)
}

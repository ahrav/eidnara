//! The source corpus under `testdata/corpus/` is the population later capture measurements run
//! over. This module owns its schema (`eidnara.source-corpus/v1`), its digest pin, the repository
//! manifest materializer, the Pi session reader that produces the expected-unit table, the redaction
//! scan over fixture strings, and the closed loss-condition enumeration.
//!
//! The reader is the oracle side of every reconciliation, so this module names no daemon item:
//! `scripts/forbid-oracle-adapter-imports.ts` rejects any path into the crate, the harness source
//! adapters, the tool outcome module, or the shared identity and block types.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) const SCHEMA: &str = "eidnara.source-corpus/v1";

/// SHA-256 of the corpus fixture files in the canonical form [`corpus_digest`] computes.
/// `packages/e2e-tests/src/source-corpus/corpus.ts` holds the same value.
pub(crate) const CORPUS_SHA256: &str =
    "7724a1dd6e2ba19b13f8a08c8d9c7f7170faef24ccaa84b34b13c986f01dea44";

/// The corpus directory; its top-level `README.md` is documentation and stays outside the digest.
pub(crate) fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/corpus")
}

/// Basenames the corpus treats as instruction documents.
pub(crate) const INSTRUCTION_DOCUMENTS: [&str; 5] = [
    "AGENTS.md",
    "CLAUDE.md",
    ".cursorrules",
    "ARCHITECTURE.md",
    "STRUCTURE.md",
];

/// Every fixture file under `dir` as `(relative path, bytes)`, sorted by path bytes. Paths use `/`
/// separators; the top-level `README.md` is skipped.
pub(crate) fn corpus_files(dir: &Path) -> std::io::Result<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        for entry in std::fs::read_dir(dir.join(&relative))? {
            let entry = entry?;
            let path = relative.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                pending.push(path);
                continue;
            }
            let name = path
                .to_str()
                .ok_or_else(|| std::io::Error::other("non-UTF-8 corpus path"))?
                .replace(std::path::MAIN_SEPARATOR, "/");
            if name != "README.md" {
                files.push((name, std::fs::read(dir.join(&path))?));
            }
        }
    }
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    Ok(files)
}

/// SHA-256 over, for each file in path-byte order, its relative path, `\n`, and its bytes with
/// every `\r\n` replaced by `\n`. The TypeScript peer computes the same form.
pub(crate) fn corpus_digest(files: &[(String, Vec<u8>)]) -> String {
    let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut hasher = Sha256::new();
    for (path, bytes) in sorted {
        hasher.update(path.as_bytes());
        hasher.update(b"\n");
        let mut rest = bytes.as_slice();
        while let Some(at) = rest.windows(2).position(|pair| pair == b"\r\n") {
            hasher.update(&rest[..at]);
            hasher.update(b"\n");
            rest = &rest[at + 2..];
        }
        hasher.update(rest);
    }
    format!("{:x}", hasher.finalize())
}

/// `Err` carries the recomputed digest when it differs from [`CORPUS_SHA256`].
pub(crate) fn verify_corpus_digest(files: &[(String, Vec<u8>)]) -> Result<(), String> {
    let actual = corpus_digest(files);
    if actual == CORPUS_SHA256 {
        Ok(())
    } else {
        Err(actual)
    }
}

/// One repository manifest under `repos/`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub schema: String,
    pub name: String,
    pub layout: Layout,
    pub instructions: Instructions,
    pub upstream: Upstream,
    pub exclusions: Vec<Exclusion>,
    pub documents: Vec<Document>,
    pub files: Vec<FileEntry>,
}

/// The R1 repository layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Layout {
    RustWorkspace,
    TypescriptMonorepo,
    PythonSrc,
    GoModule,
    Eidnara,
    NonGit,
}

/// The R1 instruction-document conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Instructions {
    /// No `AGENTS.md`, `CLAUDE.md`, or `.cursorrules` anywhere.
    None,
    /// Instruction documents at the root only.
    RootOnly,
    /// At least one `AGENTS.md` three or more path components deep.
    #[serde(rename = "nested_depth_3")]
    NestedDepth3,
    /// `CLAUDE.md` and `.cursorrules` in the same directory as an `AGENTS.md`.
    Mixed,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Upstream {
    pub repository: String,
    pub commit: String,
    pub license: String,
}

/// One blanked line of an instruction document and the reason it left the verbatim bytes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Exclusion {
    pub path: String,
    pub line: usize,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Document {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum FileEntry {
    Regular(RegularFile),
    Symlink(Symlink),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegularFile {
    pub path: String,
    pub size: u64,
    pub utf8: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Symlink {
    pub path: String,
    pub symlink: String,
}

impl FileEntry {
    pub(crate) fn path(&self) -> &str {
        match self {
            Self::Regular(file) => &file.path,
            Self::Symlink(link) => &link.path,
        }
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn check_relative(path: &str) -> Result<(), String> {
    // A `.git` entry would write into the materialized repository's administrative directory,
    // including its hooks; case-insensitive filesystems resolve `.GIT` to `.git`.
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\0')
        || path.split('/').any(|component| {
            matches!(component, "" | "." | "..") || component.eq_ignore_ascii_case(".git")
        })
    {
        return Err(format!("{path:?} is not a normalized relative path"));
    }
    Ok(())
}

fn check_sorted<'a>(kind: &str, paths: impl Iterator<Item = &'a str>) -> Result<(), String> {
    let mut previous: Option<&str> = None;
    for path in paths {
        check_relative(path)?;
        if previous.is_some_and(|previous| previous.as_bytes() >= path.as_bytes()) {
            return Err(format!(
                "{kind} {path:?} is out of path-byte order or repeated"
            ));
        }
        previous = Some(path);
    }
    Ok(())
}

impl Manifest {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, String> {
        let manifest: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA {
            return Err(format!("schema {:?}", self.schema));
        }
        let commit = &self.upstream.commit;
        if commit.len() != 40
            || !commit
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(format!(
                "upstream commit {commit:?} is not a full lowercase oid"
            ));
        }
        if !self.upstream.repository.starts_with("https://") || self.upstream.license.is_empty() {
            return Err("upstream names no https repository or no license".to_owned());
        }
        check_sorted("document", self.documents.iter().map(|d| d.path.as_str()))?;
        check_sorted("file", self.files.iter().map(FileEntry::path))?;
        let documents: BTreeSet<&str> = self.documents.iter().map(|d| d.path.as_str()).collect();
        for document in &documents {
            if !INSTRUCTION_DOCUMENTS.contains(&basename(document)) {
                return Err(format!(
                    "document {document:?} is not an instruction document"
                ));
            }
        }
        let links: BTreeSet<&str> = self
            .files
            .iter()
            .filter_map(|file| match file {
                FileEntry::Symlink(link) => Some(link.path.as_str()),
                FileEntry::Regular(_) => None,
            })
            .collect();
        let all_paths = documents
            .iter()
            .copied()
            .chain(self.files.iter().map(FileEntry::path));
        for path in all_paths {
            let mut ancestor = parent(path);
            while !ancestor.is_empty() {
                if links.contains(ancestor) {
                    return Err(format!("{path:?} lies beneath the symlink {ancestor:?}"));
                }
                ancestor = parent(ancestor);
            }
        }
        for file in &self.files {
            if documents.contains(file.path())
                || INSTRUCTION_DOCUMENTS.contains(&basename(file.path()))
            {
                return Err(format!("file {:?} is an instruction document", file.path()));
            }
            if let FileEntry::Regular(file) = file
                && file.size == 0
                && !file.utf8
            {
                return Err(format!(
                    "empty file {:?} is recorded as non-UTF-8",
                    file.path
                ));
            }
        }
        for exclusion in &self.exclusions {
            let document = self
                .documents
                .iter()
                .find(|d| d.path == exclusion.path)
                .ok_or_else(|| format!("exclusion names {:?}", exclusion.path))?;
            let line = exclusion
                .line
                .checked_sub(1)
                .and_then(|index| document.text.split('\n').nth(index));
            if line != Some("") || exclusion.reason.is_empty() {
                return Err(format!(
                    "exclusion {}:{} is not a blank line with a reason",
                    exclusion.path, exclusion.line
                ));
            }
        }
        if self.instructions_held() != Some(self.instructions) {
            return Err(format!(
                "documents hold {:?}, manifest declares {:?}",
                self.instructions_held(),
                self.instructions
            ));
        }
        Ok(())
    }

    pub(crate) fn instructions_held(&self) -> Option<Instructions> {
        let named = |name: &'static str| {
            self.documents
                .iter()
                .filter(move |d| basename(&d.path) == name)
                .map(|d| parent(&d.path))
        };
        let agents: BTreeSet<&str> = named("AGENTS.md").collect();
        let claude: BTreeSet<&str> = named("CLAUDE.md").collect();
        let directories: Vec<&str> = ["AGENTS.md", "CLAUDE.md", ".cursorrules"]
            .into_iter()
            .flat_map(named)
            .collect();
        if named(".cursorrules").any(|dir| agents.contains(dir) && claude.contains(dir)) {
            Some(Instructions::Mixed)
        } else if agents.iter().any(|dir| dir.split('/').count() >= 2) {
            Some(Instructions::NestedDepth3)
        } else if directories.is_empty() {
            Some(Instructions::None)
        } else if directories.iter().all(|dir| dir.is_empty()) {
            Some(Instructions::RootOnly)
        } else {
            None
        }
    }
}

/// Every repository manifest under `repos/`, by file name.
pub(crate) fn manifests(dir: &Path) -> Result<Vec<(String, Manifest)>, String> {
    corpus_files(dir)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|(path, _)| path.starts_with("repos/"))
        .map(|(path, bytes)| {
            let manifest = Manifest::parse(&bytes).map_err(|error| format!("{path}: {error}"))?;
            Ok((path, manifest))
        })
        .collect()
}

/// A manifest written into a temporary directory as a one-commit git repository.
pub(crate) struct Materialized {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    /// A second checkout of the same commit on its own branch, when requested.
    pub worktree: Option<PathBuf>,
}

/// Bytes regenerated for a non-document file: ASCII text when the upstream blob was UTF-8, and
/// bytes led by `0xFF` (never valid UTF-8) otherwise.
pub(crate) fn generated_bytes(size: u64, utf8: bool) -> Vec<u8> {
    const FILLER: &[u8] = b"source corpus filler\n";
    let size = usize::try_from(size).expect("a corpus file fits in memory");
    let mut bytes: Vec<u8> = FILLER.iter().copied().cycle().take(size).collect();
    if !utf8 && let Some(first) = bytes.first_mut() {
        *first = 0xFF;
    }
    bytes
}

/// Runs git with no system or global configuration and fixed identities and dates, so the same
/// manifest yields the same commit oid on every host.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(dir).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        let absolute = std::env::split_paths(&path).filter(|entry| entry.is_absolute());
        command.env("PATH", std::env::join_paths(absolute).unwrap_or_default());
    }
    let output = command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "corpus")
        .env("GIT_AUTHOR_EMAIL", "corpus@example.invalid")
        .env("GIT_AUTHOR_DATE", "@1700000000 +0000")
        .env("GIT_COMMITTER_NAME", "corpus")
        .env("GIT_COMMITTER_EMAIL", "corpus@example.invalid")
        .env("GIT_COMMITTER_DATE", "@1700000000 +0000")
        .output()
        .map_err(|error| format!("git {args:?}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {args:?} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| error.to_string())
}

pub(crate) fn materialize(manifest: &Manifest, worktree: bool) -> Result<Materialized, String> {
    let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    let root = dir.path().join("checkout");
    let target = |path: &str| -> Result<PathBuf, String> {
        let target = root.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("{path}: {error}"))?;
        }
        Ok(target)
    };
    let write = |path: &str, bytes: &[u8]| -> Result<(), String> {
        std::fs::write(target(path)?, bytes).map_err(|error| format!("{path}: {error}"))
    };
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    for document in &manifest.documents {
        write(&document.path, document.text.as_bytes())?;
    }
    for file in &manifest.files {
        match file {
            FileEntry::Regular(file) => write(&file.path, &generated_bytes(file.size, file.utf8))?,
            FileEntry::Symlink(link) => {
                std::os::unix::fs::symlink(&link.symlink, target(&link.path)?)
                    .map_err(|error| format!("{}: {error}", link.path))?;
            }
        }
    }
    // The explicit SHA-1 format makes the root commit oid independent of git's object-format default.
    git(&root, &["init", "-q", "-b", "main", "--object-format=sha1"])?;
    // `-f` adds blobs the tree's own `.gitignore` would skip; upstream tracked them.
    git(&root, &["add", "-A", "-f"])?;
    git(
        &root,
        &["commit", "-q", "--no-verify", "-m", &manifest.name],
    )?;
    let worktree = if worktree {
        let path = dir.path().join("worktree");
        let path_text = path.to_str().ok_or("non-UTF-8 temporary path")?;
        git(
            &root,
            &["worktree", "add", "-q", "-b", "corpus-worktree", path_text],
        )?;
        Some(path)
    } else {
        None
    };
    Ok(Materialized {
        _dir: dir,
        root,
        worktree,
    })
}

/// One native record of a session and what an independent reading of it expects capture to hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpectedRecord {
    /// The OpenCode message id or the Pi entry id.
    pub id: String,
    pub role: Option<String>,
    pub units: Vec<ExpectedUnit>,
    pub non_units: Vec<NonUnit>,
}

/// One capturable occurrence: a text block, or one text block of a settled tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpectedUnit {
    /// Position of the part or content item within its record.
    pub index: usize,
    pub byte_len: usize,
    pub tool: Option<ExpectedTool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpectedTool {
    pub name: String,
    pub call_id: String,
    pub is_error: bool,
    /// OpenCode `state.metadata.exit` when the harness wrote an integer; Pi writes none.
    pub exit: Option<i64>,
    /// OpenCode `state.metadata.truncated`, or Pi `details.truncation.truncated`.
    pub truncated: Option<bool>,
}

/// Native content an independent reading classifies as not a unit, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NonUnit {
    /// The part or item position; `None` when the reason covers the whole record.
    pub index: Option<usize>,
    pub reason: NonUnitReason,
    /// The native kind: the part `type`, item `type`, entry `type`, or Pi message `role`.
    pub kind: String,
}

/// Declares a closed enum whose `ALL` lists every variant once and whose `name` is its snake-case
/// label, both generated from one variant list.
macro_rules! closed_names {
    (
        $(#[$meta:meta])*
        enum $name:ident { $($(#[$doc:meta])* $variant:ident => $text:literal,)* }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        pub(crate) enum $name {
            $($(#[$doc])* $variant,)*
        }

        impl $name {
            pub(crate) const ALL: [Self; [$($text),*].len()] = [$(Self::$variant),*];

            pub(crate) const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)*
                }
            }
        }
    };
}

closed_names! {
    /// Why native content is not a unit. Closed: a reader meeting any other shape fails.
    enum NonUnitReason {
        /// An OpenCode part whose kind is neither `text` nor `tool`.
        NonTextPart => "non_text_part",
        /// An OpenCode text part marked `ignored`.
        IgnoredText => "ignored_text",
        /// OpenCode assistant text of a message without `time.completed`.
        UnsettledText => "unsettled_text",
        /// An OpenCode tool part still `pending` or `running`, or interrupted in an incomplete turn.
        UnsettledTool => "unsettled_tool",
        /// A Pi `toolResult` holding a non-text item: the whole record yields no unit.
        NonTextItem => "non_text_item",
        /// A Pi user or assistant content item other than `text`.
        SkippedItem => "skipped_item",
        /// A Pi entry whose `type` is not `message`.
        NonMessageEntry => "non_message_entry",
        /// Pi capture reads `user`, `assistant`, and `toolResult` messages; every remaining role,
        /// such as `bashExecution`, maps here.
        UncapturedRole => "uncaptured_role",
    }
}

closed_names! {
    /// The closed set of named conditions that account for an expected unit that did not publish.
    /// Reconciliation maps every missing unit to exactly one; any other outcome is unclassified
    /// loss. `docs/properties/daemon/interpretation-layer/` mirrors this list.
    enum LossCondition {
        /// The adapter refused the unit's whole native record (`adapter_refused`).
        AdapterRefused => "adapter_refused",
        /// The unit exceeds the per-unit byte cap (`units_unit_too_large`).
        UnitTooLarge => "unit_too_large",
        /// The unit was still deferred after the last replayed pass (`units_deferred`).
        Deferred => "deferred",
        /// The Kernel refused the unit as secret-bearing (`units_refused_terminal`).
        SecretBearing => "secret_bearing",
        /// The Kernel refused the unit's artifact as re-admission blocked (`units_refused_terminal`).
        ReAdmissionBlocked => "re_admission_blocked",
        /// The unit was refused with a retryable cause on the last pass (`units_refused_retryable`).
        RefusedRetryable => "refused_retryable",
        /// The unit's identity held a different digest (`units_identity_conflict`).
        IdentityConflict => "identity_conflict",
        /// A later revision of the same identity replaced the unit (`units_superseded`).
        Superseded => "superseded",
        /// The window was refused before any unit was read (`refused_*`).
        WindowRefused => "window_refused",
        /// The pass ended before publication (`kernel_error`, `deadline`, `kernel_unavailable`,
        /// `worker_panicked`, `discarded`, `session_invalidated`).
        PassFailed => "pass_failed",
    }
}

fn string<'v>(value: &'v Value, key: &str, at: &str) -> Result<&'v str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{at}: `{key}` is not a string"))
}

/// The expected-unit table of one Pi session file: one record per JSONL line, in file order.
///
/// Pi writes user and assistant text as a bare string or as `text` content items, and each
/// `toolResult` as content items under the call it answers. A tool result holding any non-text
/// item is classified whole as [`NonUnitReason::NonTextItem`].
///
/// # Errors
///
/// A line that is not JSON, or an entry missing a field Pi always writes, is a fixture defect.
pub(crate) fn read_pi_session(jsonl: &str) -> Result<Vec<ExpectedRecord>, String> {
    jsonl
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(number, line)| {
            let at = format!("line {}", number + 1);
            let entry: Value =
                serde_json::from_str(line).map_err(|error| format!("{at}: {error}"))?;
            pi_record(&entry, &at)
        })
        .collect()
}

fn pi_record(entry: &Value, at: &str) -> Result<ExpectedRecord, String> {
    let kind = string(entry, "type", at)?;
    let mut record = ExpectedRecord {
        id: string(entry, "id", at)?.to_owned(),
        role: None,
        units: Vec::new(),
        non_units: Vec::new(),
    };
    if kind != "message" {
        record.non_units.push(NonUnit {
            index: None,
            reason: NonUnitReason::NonMessageEntry,
            kind: kind.to_owned(),
        });
        return Ok(record);
    }
    let message = entry
        .get("message")
        .ok_or_else(|| format!("{at}: no `message`"))?;
    let role = string(message, "role", at)?;
    record.role = Some(role.to_owned());
    if !matches!(role, "user" | "assistant" | "toolResult") {
        record.non_units.push(NonUnit {
            index: None,
            reason: NonUnitReason::UncapturedRole,
            kind: role.to_owned(),
        });
        return Ok(record);
    }
    let items = match message.get("content") {
        Some(Value::String(text)) if role != "toolResult" => {
            record.units.push(ExpectedUnit {
                index: 0,
                byte_len: text.len(),
                tool: None,
            });
            return Ok(record);
        }
        Some(Value::Array(items)) => items,
        _ => return Err(format!("{at}: `message.content` has no supported shape")),
    };
    fn item_kind<'v>(item: &'v Value, at: &str) -> Result<&'v str, String> {
        string(item, "type", at)
    }
    if role != "toolResult" {
        for (index, item) in items.iter().enumerate() {
            match item_kind(item, at)? {
                "text" => record.units.push(ExpectedUnit {
                    index,
                    byte_len: string(item, "text", at)?.len(),
                    tool: None,
                }),
                kind => record.non_units.push(NonUnit {
                    index: Some(index),
                    reason: NonUnitReason::SkippedItem,
                    kind: kind.to_owned(),
                }),
            }
        }
        return Ok(record);
    }
    if let Some(item) = items
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) != Some("text"))
    {
        record.non_units.push(NonUnit {
            index: None,
            reason: NonUnitReason::NonTextItem,
            kind: item_kind(item, at)?.to_owned(),
        });
        return Ok(record);
    }
    let tool = ExpectedTool {
        name: string(message, "toolName", at)?.to_owned(),
        call_id: string(message, "toolCallId", at)?.to_owned(),
        is_error: message
            .get("isError")
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("{at}: `isError` is not a boolean"))?,
        exit: None,
        truncated: message
            .pointer("/details/truncation/truncated")
            .and_then(Value::as_bool),
    };
    for (index, item) in items.iter().enumerate() {
        record.units.push(ExpectedUnit {
            index,
            byte_len: string(item, "text", at)?.len(),
            tool: Some(tool.clone()),
        });
    }
    Ok(record)
}

/// Every string or object key in `value` whose redaction verdict is wrong: each is negative
/// unless it is the `text` member of an object carrying `"canary": true`, which must be positive. `at` names
/// the fixture in each finding.
pub(crate) fn redaction_violations(at: &str, value: &Value) -> Vec<String> {
    fn walk(
        at: &str,
        pointer: &mut String,
        value: &Value,
        canary_text: bool,
        out: &mut Vec<String>,
    ) {
        match value {
            Value::String(text) => {
                let verdict = context_core::redaction::detect_windowed_durable_text(text);
                match (canary_text, verdict) {
                    (false, Ok(false)) | (true, Ok(true)) => {}
                    (false, verdict) => {
                        out.push(format!("{at}{pointer}: not negative ({verdict:?})"))
                    }
                    (true, verdict) => {
                        out.push(format!("{at}{pointer}: canary not positive ({verdict:?})"))
                    }
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    let len = pointer.len();
                    pointer.push_str(&format!("/{index}"));
                    walk(at, pointer, item, false, out);
                    pointer.truncate(len);
                }
            }
            Value::Object(members) => {
                let canary = members.get("canary") == Some(&Value::Bool(true));
                if canary && !members.get("text").is_some_and(Value::is_string) {
                    out.push(format!("{at}{pointer}: canary unit has no `text` string"));
                }
                for (key, member) in members {
                    let len = pointer.len();
                    pointer.push('/');
                    pointer.push_str(key);
                    if context_core::redaction::detect_windowed_durable_text(key) != Ok(false) {
                        out.push(format!("{at}{pointer}: key not negative"));
                    }
                    walk(at, pointer, member, canary && key == "text", out);
                    pointer.truncate(len);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }
    let mut out = Vec::new();
    walk(at, &mut String::new(), value, false, &mut out);
    out
}

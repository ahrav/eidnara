//! This module confines Curator-run project-text inspection to literal path, name, and content searches and bounded reads delivered through the broker.
//!
//! The project root is held as a directory descriptor for the run. Every path the run names is validated as a relative path of ordinary components and resolved with `openat2` under `RESOLVE_BENEATH`, `RESOLVE_NO_SYMLINKS`, `RESOLVE_NO_MAGICLINKS`, and `RESOLVE_NO_XDEV`; a kernel without `openat2` fails closed. A file is opened once without side effects to learn its type, then opened again for reading and checked to be the same object, so only ordinary UTF-8 files of at most [`MAX_CAPTURE_BYTES`] are ever read. The `.eidnara` and `.git` components and the host's own store locations (by device and inode) are refused, and an inspection whose root shares a subtree with a store location is unavailable as a whole.
//!
//! A captured file becomes exact bytes in the CAS under the Curator capture retention class with a finite `retain_until`, one typed local-file observation, and an execution-hold reference charged to the run's reservation. Captures are default-Sensitive and local-only: a repository path proves nothing about provenance. Disclosure then follows the broker's ordinary read path, so every excerpt is policy-checked, render-checked, tagged, charged, and recorded in the disclosed-input union.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::ops::Range;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use kernel::{
    ArtifactErrorKind, ArtifactIngestRequest, CURATOR_CAPTURE_RETENTION_CLASS, CommitIntent,
    KernelError, KernelStore, LocalFileCaptureRequest, ProviderEgress, Sensitivity,
};
use rustix::fs::{Mode, OFlags, ResolveFlags};
use rustix::io::Errno;
use sha2::{Digest, Sha256};

use super::broker::{
    Alias, EvidenceBroker, EvidenceRead, ReferenceExpectation, Refusal, RefusalCode,
    RenderedBuffer, check_render,
};
use super::{Completeness, excerpt_window, is_capacity};

pub const MAX_CAPTURE_BYTES: u64 = 1024 * 1024;
pub const MAX_SCAN_BYTES: u64 = 16 * 1024 * 1024;
/// Directory entries one search may list; a hit is a disclosed capture, so the broker's batch and inspection bounds cap hits well below this.
pub const MAX_VISITED_ENTRIES: usize = 4096;
pub const MAX_DEPTH: usize = 16;
pub const MAX_QUERY_BYTES: usize = 256;
/// Path components refused wherever they appear: the project's own configuration and its Git store.
const REFUSED_COMPONENTS: [&str; 2] = [".eidnara", ".git"];
const RESOLVE: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
const SOURCE_KIND: &str = "local_file";
const MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// The host's own store locations, by device and inode and by canonical path.
#[derive(Debug, Default)]
pub struct ProtectedLocations {
    identities: BTreeSet<(u64, u64)>,
    roots: Vec<PathBuf>,
}

impl ProtectedLocations {
    /// Every path must exist and resolve; a store location that cannot be identified would otherwise go unprotected, so construction refuses instead.
    pub fn new(paths: impl IntoIterator<Item = PathBuf>) -> Result<Self, Refusal> {
        let mut protected = Self::default();
        for path in paths {
            let (metadata, canonical) = path
                .metadata()
                .and_then(|metadata| Ok((metadata, path.canonicalize()?)))
                .map_err(|_| refusal(RefusalCode::Unavailable))?;
            protected
                .identities
                .insert((metadata.dev(), metadata.ino()));
            protected.roots.push(canonical);
        }
        Ok(protected)
    }
}

#[derive(Debug, Clone)]
pub struct InspectionBinding {
    pub domain_id: String,
    pub scope_id: Option<String>,
    /// Each capture is created with this finite acquisition reference.
    pub retain_until: i64,
}

/// A literal query; no pattern language.
#[derive(Debug, Clone, Copy)]
pub enum SearchQuery<'a> {
    /// Files whose path relative to the root contains the literal.
    Path(&'a str),
    /// Files whose name contains the literal.
    Name(&'a str),
    /// Files whose text contains the literal.
    Content(&'a str),
}

/// One search hit: the alias of the captured file and a bounded excerpt around the match. No path, name, or digest.
#[derive(Debug)]
pub struct TextHit {
    pub alias: Alias,
    pub span: Range<u64>,
    /// The excerpt as the broker rendered, checked, tagged, and charged it.
    pub buffer: RenderedBuffer,
}

#[derive(Debug)]
pub struct SearchOutcome {
    pub hits: Vec<TextHit>,
    pub completeness: Completeness,
    /// `withheld` is true when an examined entry could not be delivered for a reason independent of the query: a refused component, type, or identity, an unreadable, oversized, non-UTF-8, or render-refused file, a truncated listing, or a store failure. A matching file that is refused looks exactly like one that does not match.
    pub withheld: bool,
}

/// One run's confined view of a project directory.
#[derive(Debug)]
pub struct ProjectText {
    root: OwnedFd,
    protected: BTreeSet<(u64, u64)>,
    binding: InspectionBinding,
    /// Captures this run already owns, by artifact digest, so one file's bytes are ingested and observed once per run.
    captured: BTreeMap<String, Captured>,
}

#[derive(Debug, Clone)]
struct Captured {
    evidence_id: String,
    byte_length: u64,
    retain_until: i64,
}

/// A file read under confinement: its bytes and the relative path it was named by.
struct ReadFile {
    relative: String,
    bytes: Vec<u8>,
}

impl ProjectText {
    /// Opens the root without following a link and refuses a root that is, contains, or lies inside a protected location.
    pub fn open(
        project_root: &Path,
        protected: &ProtectedLocations,
        binding: InspectionBinding,
    ) -> Result<Self, Refusal> {
        let canonical = project_root
            .canonicalize()
            .map_err(|_| refusal(RefusalCode::NotFound))?;
        if protected
            .roots
            .iter()
            .any(|root| root.starts_with(&canonical) || canonical.starts_with(root))
        {
            return Err(refusal(RefusalCode::Unavailable));
        }
        let root = rustix::fs::open(
            &canonical,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| refusal(RefusalCode::NotFound))?;
        let metadata = metadata(&root)?;
        if protected
            .identities
            .contains(&(metadata.dev(), metadata.ino()))
        {
            return Err(refusal(RefusalCode::Unavailable));
        }
        Ok(Self {
            root,
            protected: protected.identities.clone(),
            binding,
            captured: BTreeMap::new(),
        })
    }

    /// Captures the file at `relative_path` and discloses `range` of it through the broker.
    pub fn read(
        &mut self,
        store: &KernelStore,
        broker: &mut EvidenceBroker,
        relative_path: &str,
        range: Option<Range<u64>>,
        now_ms: i64,
    ) -> Result<EvidenceRead, Refusal> {
        admit_destination(broker)?;
        let probed = self.probe(relative_path)?;
        let file = self.read_file(relative_path, &probed)?;
        let alias = self.capture(store, broker, &file, now_ms)?;
        broker.read(store, alias.as_str(), range, now_ms)
    }

    /// Walks the root in name order and discloses one excerpt per matching file. Every bound is reported as explicit incompleteness; zero hits never prove absence. Every file the walk reaches is read and render-checked before the literal is applied, so what the outcome reveals does not depend on the literal.
    pub fn search(
        &mut self,
        store: &KernelStore,
        broker: &mut EvidenceBroker,
        query: SearchQuery<'_>,
        now_ms: i64,
    ) -> Result<SearchOutcome, Refusal> {
        admit_destination(broker)?;
        let literal = match query {
            SearchQuery::Path(text) | SearchQuery::Name(text) | SearchQuery::Content(text) => text,
        };
        if literal.is_empty() || literal.len() > MAX_QUERY_BYTES {
            return Err(refusal(RefusalCode::InvalidPath));
        }
        let mut outcome = SearchOutcome {
            hits: Vec::new(),
            completeness: Completeness::Complete,
            withheld: false,
        };
        let mut listed = 0usize;
        let mut scanned = 0u64;
        // Depth-first; children are pushed in reverse name order so the stack pops them in order.
        let mut pending = self.children(None, &mut listed, &mut outcome)?;
        pending.reverse();
        while let Some(relative) = pending.pop() {
            let Ok(probed) = self.probe(&relative) else {
                outcome.withheld = true;
                continue;
            };
            if probed.file_type().is_dir() {
                if relative.split('/').count() >= MAX_DEPTH {
                    // An unexplored subtree makes the inventory incomplete, not empty.
                    outcome.completeness = Completeness::CandidateBound;
                    outcome.withheld = true;
                    continue;
                }
                match self.children(Some(&relative), &mut listed, &mut outcome) {
                    Ok(mut children) => {
                        children.reverse();
                        pending.extend(children);
                    }
                    Err(_) => outcome.withheld = true,
                }
                continue;
            }
            if !probed.file_type().is_file() {
                outcome.withheld = true;
                continue;
            }
            let name_matches = match query {
                SearchQuery::Path(text) => relative.contains(text),
                SearchQuery::Name(text) => relative
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.contains(text)),
                SearchQuery::Content(_) => true,
            };
            if !name_matches {
                continue;
            }
            if probed.len() > MAX_CAPTURE_BYTES {
                outcome.withheld = true;
                continue;
            }
            if scanned.saturating_add(probed.len()) > MAX_SCAN_BYTES {
                return Ok(finish(outcome, Completeness::ProbeBound));
            }
            let Ok(file) = self.read_file(&relative, &probed) else {
                outcome.withheld = true;
                continue;
            };
            scanned += u64::try_from(file.bytes.len()).unwrap_or(u64::MAX);
            // Deliverability is decided before the literal is applied: a refused file is withheld whether or not it would have matched.
            if check_render(&file.bytes, None).is_err() {
                outcome.withheld = true;
                continue;
            }
            let text = String::from_utf8_lossy(&file.bytes);
            let position = match query {
                SearchQuery::Content(needle) => match text.find(needle) {
                    Some(position) => position,
                    None => continue,
                },
                SearchQuery::Path(_) | SearchQuery::Name(_) => 0,
            };
            let window = excerpt_window(&text, position);
            // Disclosing consumes one batch operation; stopping here instead of provoking the refusal keeps the run's conclusions usable.
            if broker.accounting.batch_headroom() == 0 {
                return capacity(outcome, refusal(RefusalCode::BatchLimit));
            }
            let alias = match self.capture(store, broker, &file, now_ms) {
                Ok(alias) => alias,
                Err(refusal) if is_capacity(refusal.code) => return capacity(outcome, refusal),
                Err(_) => {
                    outcome.withheld = true;
                    continue;
                }
            };
            let span = u64::try_from(window.start).unwrap_or(u64::MAX)
                ..u64::try_from(window.end).unwrap_or(u64::MAX);
            match broker.read(store, alias.as_str(), Some(span.clone()), now_ms) {
                Ok(read) => outcome.hits.push(TextHit {
                    alias,
                    span,
                    buffer: read.buffer,
                }),
                Err(refusal) if is_capacity(refusal.code) => return capacity(outcome, refusal),
                Err(_) => outcome.withheld = true,
            }
        }
        Ok(outcome)
    }

    /// The relative paths of the entries under `directory` (the root when `None`), in name order. Every name read counts against [`MAX_VISITED_ENTRIES`]; a listing cut short by that bound marks the outcome incomplete and withheld. Refused components and names that are not UTF-8 are withheld.
    fn children(
        &self,
        directory: Option<&str>,
        listed: &mut usize,
        outcome: &mut SearchOutcome,
    ) -> Result<Vec<String>, Refusal> {
        let handle = match directory {
            Some(relative) => self.open_beneath(relative, OFlags::RDONLY | OFlags::DIRECTORY)?,
            None => self
                .root
                .try_clone()
                .map_err(|_| refusal(RefusalCode::Unavailable))?,
        };
        let mut names = Vec::new();
        for entry in rustix::fs::Dir::read_from(handle.as_fd())
            .map_err(|_| refusal(RefusalCode::Unavailable))?
        {
            let entry = entry.map_err(|_| refusal(RefusalCode::Unavailable))?;
            let Ok(name) = entry.file_name().to_str() else {
                outcome.withheld = true;
                continue;
            };
            if name == "." || name == ".." {
                continue;
            }
            if *listed >= MAX_VISITED_ENTRIES {
                outcome.completeness = Completeness::CandidateBound;
                outcome.withheld = true;
                break;
            }
            *listed += 1;
            if REFUSED_COMPONENTS.contains(&name) {
                outcome.withheld = true;
                continue;
            }
            names.push(match directory {
                Some(relative) => format!("{relative}/{name}"),
                None => name.to_string(),
            });
        }
        names.sort();
        Ok(names)
    }

    /// Learns an entry's type and size through a side-effect-free `O_PATH` open under confinement.
    fn probe(&self, relative: &str) -> Result<std::fs::Metadata, Refusal> {
        let handle = self.open_beneath(relative, OFlags::PATH)?;
        metadata(&handle)
    }

    /// Reads one ordinary file under confinement. `probed` comes from [`Self::probe`]; the file is opened again for reading and must be the same object, so a swap between the two opens is refused rather than read. A file with more than one hard link is not ordinary: a link can name a protected file the identity set does not list.
    fn read_file(&self, relative: &str, probed: &std::fs::Metadata) -> Result<ReadFile, Refusal> {
        if !probed.file_type().is_file() {
            return Err(refusal(RefusalCode::NotRegularFile));
        }
        if probed.len() > MAX_CAPTURE_BYTES {
            return Err(refusal(RefusalCode::TooLarge));
        }
        let handle = self.open_beneath(relative, OFlags::RDONLY | OFlags::NONBLOCK)?;
        let opened = metadata(&handle)?;
        if (opened.dev(), opened.ino()) != (probed.dev(), probed.ino())
            || !opened.file_type().is_file()
        {
            return Err(refusal(RefusalCode::Confinement));
        }
        if opened.nlink() > 1 {
            return Err(refusal(RefusalCode::NotRegularFile));
        }
        let mut bytes = Vec::new();
        // The size is checked again on the bytes read: the file can grow between the stat and the read.
        File::from(handle)
            .take(MAX_CAPTURE_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| refusal(RefusalCode::Unavailable))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CAPTURE_BYTES {
            return Err(refusal(RefusalCode::TooLarge));
        }
        std::str::from_utf8(&bytes).map_err(|_| refusal(RefusalCode::Undecodable))?;
        Ok(ReadFile {
            relative: relative.to_string(),
            bytes,
        })
    }

    /// Resolves `relative` beneath the root with every resolve restriction; the result's identity is checked against the protected locations.
    fn open_beneath(&self, relative: &str, flags: OFlags) -> Result<OwnedFd, Refusal> {
        validate_relative(relative)?;
        let handle = rustix::fs::openat2(
            self.root.as_fd(),
            relative,
            flags | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            RESOLVE,
        )
        .map_err(|errno| refusal(open_refusal(errno)))?;
        let metadata = metadata(&handle)?;
        if self.protected.contains(&(metadata.dev(), metadata.ino())) {
            return Err(refusal(RefusalCode::Protected));
        }
        Ok(handle)
    }

    /// Records `file` as owned evidence once per run and issues its alias. The bytes must survive ingestion unchanged, so a buffer the scanner would rewrite is refused before anything is stored and ingestion itself is exact; the path is checked the same way because it is stored in the typed detail.
    fn capture(
        &mut self,
        store: &KernelStore,
        broker: &mut EvidenceBroker,
        file: &ReadFile,
        now_ms: i64,
    ) -> Result<Alias, Refusal> {
        check_render(&file.bytes, None)?;
        check_render(file.relative.as_bytes(), None)?;
        let digest = format!("{:x}", Sha256::digest(&file.bytes));
        let byte_length = u64::try_from(file.bytes.len()).unwrap_or(u64::MAX);
        let captured = match self.captured.get(&digest) {
            Some(captured) => captured.clone(),
            None => {
                let hold = &broker.binding().hold;
                let evidence_id = format!("curcap:{}:{}:{digest}", hold.subject, hold.generation);
                let handle = store
                    .ingest_exact_artifact(ArtifactIngestRequest {
                        intent: intent(&evidence_id, &digest),
                        payload: file.bytes.clone(),
                        evidence_id: evidence_id.clone(),
                        object_id: format!("curcapobj:{evidence_id}"),
                        object_kind: "evidence".to_string(),
                        domain_id: self.binding.domain_id.clone(),
                        source_kind: SOURCE_KIND.to_string(),
                        source_id: file.relative.clone(),
                        source_revision: 1,
                        media_type: MEDIA_TYPE.to_string(),
                        retention_class: CURATOR_CAPTURE_RETENTION_CLASS.to_string(),
                        retain_until: Some(self.binding.retain_until),
                        asserted_sensitivity: Sensitivity::Sensitive,
                        provider_egress: ProviderEgress::LocalOnly,
                        provenance: None,
                    })
                    .map_err(|error| {
                        refusal(match error.kind() {
                            ArtifactErrorKind::ExactBytesRewritten => RefusalCode::RenderCheck,
                            ArtifactErrorKind::UnsupportedShape => RefusalCode::Undecodable,
                            _ => RefusalCode::Store,
                        })
                    })?;
                if handle.digest != digest {
                    return Err(refusal(RefusalCode::Store));
                }
                store
                    .commit(
                        intent(&format!("{evidence_id}:observation"), &digest),
                        |envelope| {
                            envelope.record_local_file_capture(&LocalFileCaptureRequest {
                                project_digest: &broker.binding().hold.project_digest,
                                relative_path: &file.relative,
                                captured_at: now_ms,
                                domain_id: &self.binding.domain_id,
                                scope_id: self.binding.scope_id.as_deref(),
                                evidence_id: &evidence_id,
                                artifact_digest: &digest,
                                byte_length,
                            })?;
                            Ok(String::new())
                        },
                    )
                    .map_err(|error| {
                        refusal(match error {
                            KernelError::InvalidInput => RefusalCode::RenderCheck,
                            _ => RefusalCode::Store,
                        })
                    })?;
                // Ownership is charged to the run's reservation at first capture, not deferred to the first disclosure. The held facts, not the request, define the alias: a replayed ingest keeps the row's original `retain_until`.
                let held = broker.hold_evidence(store, None, &evidence_id, now_ms)?;
                let captured = Captured {
                    evidence_id,
                    byte_length: held.byte_length,
                    retain_until: held
                        .retain_until
                        .ok_or_else(|| refusal(RefusalCode::Store))?,
                };
                self.captured.insert(digest.clone(), captured.clone());
                captured
            }
        };
        Ok(broker
            .aliases
            .issue(ReferenceExpectation::TemporaryCapture {
                evidence_id: captured.evidence_id,
                artifact_digest: digest,
                byte_length: captured.byte_length,
                retain_until: captured.retain_until,
            }))
    }
}

fn finish(mut outcome: SearchOutcome, completeness: Completeness) -> SearchOutcome {
    outcome.completeness = completeness;
    outcome
}

/// A run bound reached after this search disclosed something ends the search with what it has; with nothing disclosed the refusal itself is the answer.
fn capacity(outcome: SearchOutcome, refusal: Refusal) -> Result<SearchOutcome, Refusal> {
    if outcome.hits.is_empty() {
        return Err(refusal);
    }
    Ok(finish(outcome, Completeness::CapacityBound))
}

fn metadata(handle: &OwnedFd) -> Result<std::fs::Metadata, Refusal> {
    File::from(
        handle
            .try_clone()
            .map_err(|_| refusal(RefusalCode::Unavailable))?,
    )
    .metadata()
    .map_err(|_| refusal(RefusalCode::Unavailable))
}

/// A capture is Sensitive by construction, so a remote destination can never disclose one; refusing at entry reads nothing for a run that could not be shown it.
fn admit_destination(broker: &EvidenceBroker) -> Result<(), Refusal> {
    if broker.binding().destination == kernel::ArtifactDestination::Remote {
        return Err(refusal(RefusalCode::PolicyBlocked));
    }
    Ok(())
}

/// A relative path of at most [`MAX_DEPTH`] ordinary components: no root, no `.` or `..`, no empty component, no NUL, and no refused component.
fn validate_relative(relative: &str) -> Result<(), Refusal> {
    if relative.is_empty() || relative.starts_with('/') || relative.contains('\0') {
        return Err(refusal(RefusalCode::InvalidPath));
    }
    let mut depth = 0;
    for component in relative.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(refusal(RefusalCode::InvalidPath));
        }
        if REFUSED_COMPONENTS.contains(&component) {
            return Err(refusal(RefusalCode::Protected));
        }
        depth += 1;
    }
    if depth > MAX_DEPTH {
        return Err(refusal(RefusalCode::InvalidPath));
    }
    Ok(())
}

/// Maps an `openat2` failure to a bounded code. A kernel that lacks the call or one of its resolve flags fails closed; only a missing entry is `NotFound`, so a host failure is never reported as absence.
fn open_refusal(errno: Errno) -> RefusalCode {
    match errno {
        Errno::NOSYS | Errno::INVAL | Errno::OPNOTSUPP => RefusalCode::Unsupported,
        Errno::XDEV | Errno::LOOP | Errno::AGAIN => RefusalCode::Confinement,
        Errno::NOTDIR => RefusalCode::NotRegularFile,
        Errno::NOENT => RefusalCode::NotFound,
        _ => RefusalCode::Unavailable,
    }
}

fn intent(operation_key: &str, digest: &str) -> CommitIntent {
    CommitIntent {
        producer: "curator".to_string(),
        operation_key: operation_key.to_string(),
        request_digest: digest.to_string(),
        actor: "curator".to_string(),
        cause: "project_text_capture".to_string(),
    }
}

fn refusal(code: RefusalCode) -> Refusal {
    Refusal { alias: None, code }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_ordinary_components_only() {
        for accepted in ["a", "a/b.txt", "deep/er/path"] {
            validate_relative(accepted).unwrap();
        }
        for (rejected, code) in [
            ("", RefusalCode::InvalidPath),
            ("/etc/passwd", RefusalCode::InvalidPath),
            ("a/../b", RefusalCode::InvalidPath),
            ("./a", RefusalCode::InvalidPath),
            ("a//b", RefusalCode::InvalidPath),
            ("a\0b", RefusalCode::InvalidPath),
            (".git/config", RefusalCode::Protected),
            ("src/.eidnara/eidnara.jsonc", RefusalCode::Protected),
        ] {
            assert_eq!(
                validate_relative(rejected).unwrap_err().code,
                code,
                "{rejected:?}"
            );
        }
        let deep = vec!["d"; MAX_DEPTH + 1].join("/");
        assert_eq!(
            validate_relative(&deep).unwrap_err().code,
            RefusalCode::InvalidPath
        );
    }

    #[test]
    fn open_failures_fail_closed_and_never_report_a_host_failure_as_absence() {
        for (errno, code) in [
            (Errno::NOSYS, RefusalCode::Unsupported),
            (Errno::INVAL, RefusalCode::Unsupported),
            (Errno::OPNOTSUPP, RefusalCode::Unsupported),
            (Errno::XDEV, RefusalCode::Confinement),
            (Errno::LOOP, RefusalCode::Confinement),
            (Errno::AGAIN, RefusalCode::Confinement),
            (Errno::NOTDIR, RefusalCode::NotRegularFile),
            (Errno::NOENT, RefusalCode::NotFound),
            (Errno::ACCESS, RefusalCode::Unavailable),
            (Errno::MFILE, RefusalCode::Unavailable),
            (Errno::IO, RefusalCode::Unavailable),
        ] {
            assert_eq!(open_refusal(errno), code, "{errno:?}");
        }
    }
}

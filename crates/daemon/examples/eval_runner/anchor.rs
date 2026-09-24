//! The real-history anchor shell: clones each anchor repository, builds the
//! cutoff snapshot, audits the cutoff, runs the current-tree-only
//! insufficiency proof, runs the no-repository control per provider inside
//! the Suite D containment, and publishes the per-provider accounting.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use context_core::canonical_json::protocol_digest;
use eval_core::{
    Affordability, AnchorCorpus, AnchorEntry, AnchorError, AnchorRole, Approval, CensorReason,
    ClaimBoundary, ClaimDerivation, ClassifiedControl, ControlRefused, CutoffAudit, CutoffRefused,
    Family, FrozenFamily, HiddenOutcome, HiddenResults, InsufficiencyProof, NoRepositoryControl,
    PairAccounting, Preparation, ProfileError, ProviderProfile, RealHistorySettings,
    RealHistorySkip, RealHistoryUnsupported, RepositoryComparison, RunProfile, Scale,
    SettingsRefused, TaskBudgets, TaskUsage, Terminal, TimeStudyRefused, TransferCriterion,
    WitnessError, WorldProvenance, anchor_set, classify_control, eval_run_id, future_answers,
    hidden_terminal, parse_witness, time_study,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::aging::{ManifestInputs, suite_c_manifest};
use super::campaign::{Charges, identity, prepare_publish, publish_file, sha256_hex};
use super::fault::ChildGuard;
use super::suite_d::{self, GradeCache, charged_run, contain, run_bounded, run_hidden};

pub const SIMULATOR_VERSION: &str = "eval-anchor-shell/v1";
pub const ANCHOR_REPORT_SCHEMA: &str = "eval-anchor-report/v1";
pub const SNAPSHOT_DIGEST_PROTOCOL: &str = "eval-anchor-snapshot/v2";
pub const REPORT_FILE: &str = "anchor-report.json";
pub const MANIFEST_FILE: &str = "manifest.json";
const TOOL_LINE: &str = "eval-anchor-tool";
const GIT_TIMEOUT: Duration = Duration::from_secs(120);
const GRADE_TIMEOUT: Duration = Duration::from_secs(600);

/// What the runner fetched at run time for one entry and never persists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub issue_text: String,
    pub issue_created_ms: i64,
    /// When `issue_text` was written: the issue's last edit, or its creation
    /// when it was never edited.
    pub issue_text_ms: i64,
    /// When the pull request was opened, when the entry names one and the
    /// host knows.
    pub pull_request_created_ms: Option<i64>,
}

/// The host seams: how a repository is cloned, how an issue is fetched, and
/// whether the host can create the containment's namespaces.
#[derive(Debug, Clone, Copy)]
pub struct Host {
    pub clone: fn(&AnchorEntry, &Path) -> std::io::Result<()>,
    pub fetch: fn(&AnchorEntry) -> Option<Fetched>,
    pub namespaces: fn() -> bool,
}

/// What the scripted control agent does from the statement alone.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct ControlScript {
    /// Write the fix from memory: the memorized case.
    pub memorize: bool,
    /// Read the repository snapshot it was not given: seeded contamination.
    pub reach_repository: bool,
    /// Name the pull request in its output: seeded future knowledge.
    pub cite_future: bool,
    pub stall: bool,
    pub exit_code: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub scale: Scale,
    pub elapsed_bound_ms: u64,
    pub approval: Option<Approval>,
    pub witness: PathBuf,
    pub publish: PathBuf,
    pub corpus: AnchorCorpus,
    pub settings: RealHistorySettings,
    /// Frozen into the analysis family; absent, every claim derives as
    /// `generated_phase1`.
    pub transfer_criterion: Option<TransferCriterion>,
    pub control: ControlScript,
    pub budgets: TaskBudgets,
    pub store_bound_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("profile refused: {0}")]
    Profile(#[from] ProfileError),
    #[error("settings refused: {0}")]
    Settings(#[from] SettingsRefused),
    #[error("corpus refused: {0}")]
    Corpus(#[from] AnchorError),
    #[error("the Phase 5 witness was not accepted: {0}")]
    Witness(#[from] WitnessError),
    #[error("time study refused: {0}")]
    TimeStudy(#[from] TimeStudyRefused),
    #[error("the pilot is not affordable; stopping for maintainer approval: {0:?}")]
    StopForApproval(Affordability),
    #[error("this host cannot create the containment's namespaces; nothing can run")]
    NoContainment,
    #[error("the containment refused to mount while grading {task}")]
    MountRefused { task: String },
    #[error("suite d shell: {0}")]
    SuiteD(#[from] suite_d::RunError),
    #[error("envelope exceeded: {0:?}")]
    Envelope(#[from] eval_core::EnvelopeExceeded),
    #[error("the control for {task} exited with {code:?}")]
    ControlExited { task: String, code: Option<i32> },
    #[error("the control for {task} is not comparable: {refused}")]
    NotComparable {
        task: String,
        refused: ControlRefused,
    },
    #[error("publish refused at {}: {kind}", path.display())]
    Publish {
        path: PathBuf,
        kind: std::io::ErrorKind,
    },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// How a real-history task ended: the shared outcomes `hidden_terminal`
/// yields, or a skip or unsupported reason of the real-history contract's
/// own; the shared v1 `SkipReason` and `UnsupportedReason` stay closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnchorTerminal {
    Pass,
    Fail,
    Censored { reason: CensorReason },
    Indeterminate,
    Skipped(RealHistorySkip),
    Unsupported(RealHistoryUnsupported),
}

impl AnchorTerminal {
    /// The shared terminal a graded run yields; it never yields a skip, an
    /// unsupported, or a disabled kind, so those are not representable.
    fn judged(terminal: Terminal) -> Self {
        match terminal {
            Terminal::Pass => Self::Pass,
            Terminal::Fail => Self::Fail,
            Terminal::Censored { reason } => Self::Censored { reason },
            Terminal::Indeterminate => Self::Indeterminate,
            other => panic!("a graded run yields no {other:?}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskOutcome {
    pub id: String,
    pub terminal: AnchorTerminal,
    #[serde(with = "eval_core::decimal")]
    pub prepare_ms: u64,
    pub audit: Option<CutoffAudit>,
    pub audit_refused: Option<CutoffRefused>,
    pub insufficiency: Option<InsufficiencyProof>,
    /// One control per provider pair, each naming its provider.
    pub controls: Vec<NoRepositoryControl>,
    pub verdicts: Vec<ClassifiedControl>,
}

/// One provider pair's claim, next to the profile it was derived for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairClaim {
    pub provider: ProviderProfile,
    pub claim: ClaimDerivation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorReport {
    pub schema: String,
    pub eval_run_id: String,
    pub profile_digest: String,
    pub claim_boundary: ClaimBoundary,
    pub corpus_digest: String,
    pub role: AnchorRole,
    pub time_study: Affordability,
    pub tasks: Vec<TaskOutcome>,
    /// Per provider pair, in the settings' order; each names its provider.
    pub accounting: Vec<PairAccounting>,
    pub claims: Vec<PairClaim>,
    pub envelope: eval_core::Envelope,
}

pub struct Run {
    pub report: AnchorReport,
    pub report_bytes: Vec<u8>,
    pub manifest: eval_core::Manifest,
    pub manifest_bytes: Vec<u8>,
}

fn git(dir: &Path, args: &[&str], deadline: Duration) -> std::io::Result<Option<String>> {
    let mut command = Command::new("git");
    command.args(args).current_dir(dir).stderr(Stdio::null());
    let (status, out) =
        run_bounded(command, deadline).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(status.filter(ExitStatus::success).map(|_| out))
}

/// The paths `git diff` reports between two commits, added or changed
/// according to `filter`; `-z` preserves unusual paths without Git's
/// line-oriented quoting.
fn diff_paths(
    repo: &Path,
    filter: &str,
    from: &str,
    to: &str,
    deadline: Duration,
) -> std::io::Result<Vec<String>> {
    let filter = format!("--diff-filter={filter}");
    let out = git(
        repo,
        &[
            "diff",
            "-z",
            "--name-only",
            "--no-renames",
            &filter,
            from,
            to,
        ],
        deadline,
    )?
    .unwrap_or_default();
    Ok(out
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect())
}

pub fn pipe_bounded(
    mut producer: Command,
    mut consumer: Command,
    deadline: Duration,
) -> std::io::Result<Option<bool>> {
    let started = Instant::now();
    let mut first = ChildGuard(
        producer
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let pipe = first.0.stdout.take().unwrap();
    let mut second = ChildGuard(
        consumer
            .stdin(Stdio::from(pipe))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let (mut a, mut b) = (None, None);
    while a.is_none() || b.is_none() {
        if a.is_none() {
            a = first.0.try_wait()?;
        }
        if b.is_none() {
            b = second.0.try_wait()?;
        }
        if started.elapsed() >= deadline && (a.is_none() || b.is_none()) {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(Some(a.unwrap().success() && b.unwrap().success()))
}

fn archive(repo: &Path, rev: &str, into: &Path, deadline: Duration) -> std::io::Result<bool> {
    let mut producer = Command::new("git");
    producer
        .args(["archive", "--format=tar", rev])
        .current_dir(repo);
    let mut consumer = Command::new("tar");
    consumer.args(["-x", "-f", "-", "-C"]).arg(into);
    Ok(pipe_bounded(producer, consumer, deadline)? == Some(true))
}

/// `cp -RP` keeps bytes, symlinks, and modes but not timestamps: a file's
/// mtime must be newer than the build artifacts in the shared target
/// directory, or Cargo reuses a build of the tree it replaced.
fn copy_tree(from: &Path, into: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(into)?;
    let status = Command::new("cp")
        .arg("-RP")
        .arg("--")
        .arg(from.join("."))
        .arg(into)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "cp -RP {} {} failed",
            from.display(),
            into.display()
        )))
    }
}

fn fresh_dir(path: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path).is_ok() {
        std::fs::remove_dir_all(path)?;
    }
    std::fs::create_dir_all(path)
}

fn regular_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() && entry.file_name() != ".git" {
                walk(root, &path, out)?;
            } else if kind.is_file() {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    if root.is_dir() {
        walk(root, root, &mut out)?;
    }
    Ok(out)
}

/// Symlinks in `patch` are not followed. A file is skipped when a directory
/// on its path in `into` is a symlink.
fn overlay_patch(patch: &Path, into: &Path) -> std::io::Result<()> {
    'files: for relative in regular_files(patch)? {
        let mut dir = into.to_path_buf();
        for component in relative.parent().into_iter().flat_map(Path::components) {
            dir.push(component);
            match std::fs::symlink_metadata(&dir) {
                Ok(meta) if meta.file_type().is_symlink() => continue 'files,
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => std::fs::remove_file(&dir)?,
                Err(_) => {}
            }
        }
        std::fs::create_dir_all(&dir)?;
        let target = into.join(&relative);
        match std::fs::symlink_metadata(&target) {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&target)?,
            Ok(meta) if !meta.is_file() => std::fs::remove_file(&target)?,
            _ => {}
        }
        std::fs::copy(patch.join(&relative), target)?;
    }
    Ok(())
}

fn tree_digest(root: &Path) -> std::io::Result<String> {
    use std::os::unix::fs::PermissionsExt;
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Value>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .to_string();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                if relative != ".git" {
                    walk(root, &path, out)?;
                }
            } else if kind.is_symlink() {
                let target = std::fs::read_link(&path)?;
                out.insert(
                    relative,
                    json!({"kind": "symlink", "target": target.to_string_lossy()}),
                );
            } else {
                let mut hasher = Sha256::new();
                let mut file = std::fs::File::open(&path)?;
                let mut chunk = [0u8; 65_536];
                loop {
                    let n = file.read(&mut chunk)?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&chunk[..n]);
                }
                let executable = entry.metadata()?.permissions().mode() & 0o111 != 0;
                out.insert(
                    relative,
                    json!({
                        "kind": "file",
                        "executable": executable,
                        "sha256": format!("{:x}", hasher.finalize()),
                    }),
                );
            }
        }
        Ok(())
    }
    let mut entries = BTreeMap::new();
    walk(root, root, &mut entries)?;
    Ok(protocol_digest(SNAPSHOT_DIGEST_PROTOCOL, &json!(entries)).unwrap())
}

/// Only a file directly under `tests/` is a Cargo test target by itself; a
/// file deeper down is a module of some other target.
fn hidden_test_name(path: &str) -> Option<&str> {
    path.strip_prefix("tests/")?
        .strip_suffix(".rs")
        .filter(|name| !name.is_empty() && !name.contains('/'))
}

fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

struct Prepared {
    snapshot: PathBuf,
    /// The fix commit's whole tree, its hidden tests removed.
    fix: PathBuf,
    audit: CutoffAudit,
    hidden: Vec<(String, String)>,
    fetched: Fetched,
}

/// The digest of `rev`'s tree read the same way as the snapshot's: archived
/// into `scratch` and digested file by file.
fn revision_tree_digest(
    repo: &Path,
    rev: &str,
    scratch: &Path,
    deadline: Duration,
) -> std::io::Result<Option<String>> {
    fresh_dir(scratch)?;
    let digest = if archive(repo, rev, scratch, deadline)? {
        Some(tree_digest(scratch)?)
    } else {
        None
    };
    Ok(digest)
}

/// Clones, archives the base into the snapshot and the whole fix into the
/// fix tree, reads the commit times and tree digests the audit needs, and
/// removes the clone and the scratch tree. The store and the elapsed bound
/// are charged after the clone and after each tree is extracted, while all
/// of them still exist, so a checkout or a tree larger than the bound stops
/// the run here; every git child's deadline is the campaign time left.
fn prepare(
    entry: &AnchorEntry,
    host: Host,
    tasks: &Path,
    charges: &mut Charges,
    store_root: &Path,
) -> Result<Result<Prepared, AnchorTerminal>, RunError> {
    let unavailable = || {
        Ok(Err(AnchorTerminal::Unsupported(
            RealHistoryUnsupported::SourceUnavailable,
        )))
    };
    let repo = tasks.join("repos").join(&entry.id);
    let scratch = tasks.join("scratch");
    fresh_dir(&repo)?;
    let prepared = (|| -> Result<Result<Prepared, AnchorTerminal>, RunError> {
        charges.elapsed()?;
        if (host.clone)(entry, &repo).is_err() {
            return unavailable();
        }
        charges.store_bytes(store_root)?;
        charges.elapsed()?;
        let deadline =
            GIT_TIMEOUT.min(charges.deadline().saturating_duration_since(Instant::now()));
        let Some(fetched) = (host.fetch)(entry) else {
            return unavailable();
        };
        let committed = |sha: &str| -> std::io::Result<Option<i64>> {
            Ok(git(&repo, &["show", "-s", "--format=%ct", sha], deadline)?
                .and_then(|out| out.trim().parse::<i64>().ok())
                .map(|seconds| seconds * 1_000))
        };
        let (Some(base_ms), Some(fix_ms)) =
            (committed(&entry.base_sha)?, committed(&entry.fix_sha)?)
        else {
            return unavailable();
        };
        let parent = format!("{}^", entry.fix_sha);
        let Some(parent_sha) = git(&repo, &["rev-parse", "--verify", &parent], deadline)? else {
            return unavailable();
        };
        let parent_sha = parent_sha.trim().to_string();
        let descends = git(
            &repo,
            &[
                "merge-base",
                "--is-ancestor",
                &entry.base_sha,
                &entry.fix_sha,
            ],
            deadline,
        )?
        .is_some();
        // The repair became public no later than the earliest fix-side
        // commit and, when known, the pull request's creation.
        let range = format!("{}..{}", entry.base_sha, entry.fix_sha);
        let fix_side_ms = git(&repo, &["log", "--format=%ct", &range], deadline)?
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().parse::<i64>().ok())
            .map(|seconds| seconds * 1_000)
            .min()
            .unwrap_or(fix_ms);
        let repair_public_ms = fetched
            .pull_request_created_ms
            .map_or(fix_side_ms, |pr_ms| pr_ms.min(fix_side_ms));
        let snapshot = tasks.join("snapshots").join(&entry.id);
        fresh_dir(&snapshot)?;
        if !archive(&repo, &entry.base_sha, &snapshot, deadline)? {
            return unavailable();
        }
        charges.store_bytes(store_root)?;
        let fix = tasks.join("fixes").join(&entry.id);
        fresh_dir(&fix)?;
        if !archive(&repo, &entry.fix_sha, &fix, deadline)? {
            return unavailable();
        }
        charges.store_bytes(store_root)?;
        let fix_tree_digest = tree_digest(&fix)?;
        let Some(fix_parent_tree_digest) =
            revision_tree_digest(&repo, &parent_sha, &scratch, deadline)?
        else {
            return unavailable();
        };
        charges.store_bytes(store_root)?;
        charges.elapsed()?;
        // The patch and the hidden tests are what the fix commit itself
        // changed against its parent; intervening history is not the fix.
        let added = diff_paths(&repo, "A", &parent_sha, &entry.fix_sha, deadline)?;
        let mut hidden = Vec::new();
        for path in &added {
            let Some(name) = hidden_test_name(path) else {
                continue;
            };
            if let Ok(content) = std::fs::read_to_string(fix.join(path)) {
                std::fs::remove_file(fix.join(path))?;
                hidden.push((name.to_string(), content));
            }
        }
        let snapshot_digest = tree_digest(&snapshot)?;
        let audit = CutoffAudit {
            task: entry.id.clone(),
            entry_digest: entry.digest()?,
            cutoff_ms: entry.cutoff_ms,
            base_committed_ms: base_ms,
            fix_committed_ms: fix_ms,
            repair_public_ms,
            issue_created_ms: fetched.issue_created_ms,
            issue_text_ms: fetched.issue_text_ms,
            base_tree_digest: snapshot_digest.clone(),
            snapshot_digest,
            fix_tree_digest,
            fix_parent_tree_digest,
            fix_descends_from_base: descends,
        };
        Ok(Ok(Prepared {
            snapshot,
            fix,
            audit,
            hidden,
            fetched,
        }))
    })();
    std::fs::remove_dir_all(&repo)?;
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch)?;
    }
    prepared
}

/// Writes `tests` as `tests/hidden_<name>.rs` into `tree` (a `tests` entry
/// that is not a directory and a symlink at a test's path are removed first,
/// `.cargo/` is dropped) and grades them inside the containment, the tree
/// read-only, only `layout.target` writable, and `layout.tasks` (every
/// snapshot and fix tree) covered by an empty tmpfs.
pub fn grade(
    tree: &Path,
    tests: &[(String, String)],
    layout: &Layout,
    task: &str,
    charges: &mut Charges,
) -> Result<HiddenResults, RunError> {
    let _ = std::fs::remove_dir_all(tree.join(".cargo"));
    let dir = tree.join("tests");
    if std::fs::symlink_metadata(&dir).is_ok_and(|meta| !meta.is_dir()) {
        std::fs::remove_file(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    for (name, content) in tests {
        let path = dir.join(format!("hidden_{name}.rs"));
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_symlink()) {
            std::fs::remove_file(&path)?;
        }
        std::fs::write(path, content)?;
    }
    let names: Vec<String> = tests.iter().map(|(name, _)| name.clone()).collect();
    let tmp = layout.target.join("tmp");
    let cargo_home = layout.private.join("cargo-home");
    std::fs::create_dir_all(&tmp)?;
    std::fs::create_dir_all(&cargo_home)?;
    let cache = GradeCache {
        target: &layout.target,
        cargo_home: &cargo_home,
        tmp: &tmp,
        mask: Some(&layout.tasks),
    };
    // A repository's own lockfile is kept and held to; a tree without one
    // gets one written by the runner.
    let keep_lockfile = tree.join("Cargo.lock").is_file();
    match run_hidden(
        tree,
        &names,
        cache,
        keep_lockfile,
        true,
        GRADE_TIMEOUT,
        charges,
    )? {
        Some(graded) if graded.mount_refused => Err(RunError::MountRefused {
            task: task.to_string(),
        }),
        Some(graded) => Ok(graded.hidden),
        // The manifest did not resolve offline: nothing was executed.
        None => Ok(names
            .into_iter()
            .map(|name| (name, HiddenOutcome::Errored))
            .collect()),
    }
}

/// The control's containment mounts an empty tmpfs over `private`, hiding
/// all but `root`'s control workspace; grading runs with `tasks` (every
/// clone, snapshot, and fix tree) covered the same way, the one tree being
/// graded readable, and only `target` writable.
pub struct Layout {
    pub root: PathBuf,
    pub private: PathBuf,
    pub tasks: PathBuf,
    pub tree: PathBuf,
    pub target: PathBuf,
}

/// The control: the statement alone in an otherwise empty workspace inside
/// the containment, then the patch it wrote graded on a fresh snapshot.
struct ControlRun<'a> {
    entry: &'a AnchorEntry,
    prepared: &'a Prepared,
    provider: &'a ProviderProfile,
    config: &'a Config,
    digest: &'a str,
    layout: &'a Layout,
}

fn control(run: &ControlRun<'_>, charges: &mut Charges) -> Result<NoRepositoryControl, RunError> {
    let ControlRun {
        entry,
        prepared,
        provider,
        config,
        digest,
        layout,
    } = *run;
    let workspace = layout.root.join("control");
    fresh_dir(&workspace)?;
    std::fs::create_dir_all(workspace.join("patch"))?;
    std::fs::write(workspace.join("STATEMENT.md"), &prepared.fetched.issue_text)?;
    let tool = |name: &str, argument: &str, command: &str| {
        format!(
            "echo {}\n{command}",
            sh_quote(&format!("{TOOL_LINE} {name} {argument}"))
        )
    };
    let mut lines = vec![
        "set -e".to_string(),
        tool("cat", "STATEMENT.md", "cat STATEMENT.md >/dev/null"),
    ];
    if config.control.memorize {
        copy_tree(&prepared.fix, &workspace.join(".memory"))?;
        for path in regular_files(&prepared.fix)? {
            lines.push(tool("write", &format!("patch/{}", path.display()), "true"));
        }
        lines.push("cp -a .memory/. patch/ && rm -rf .memory".to_string());
    }
    if config.control.reach_repository {
        let relative = Path::new("..")
            .join(prepared.snapshot.strip_prefix(&layout.root).unwrap())
            .join("src/lib.rs");
        let relative = relative.to_string_lossy();
        lines.push(tool(
            "cat",
            &relative,
            &format!(
                "cp {} patch/src/lib.rs 2>/dev/null || true",
                sh_quote(&relative)
            ),
        ));
    }
    if config.control.cite_future {
        lines.push(format!(
            "echo 'this was fixed in #{}'",
            entry.pull_request.unwrap_or(0)
        ));
    }
    if config.control.stall {
        lines.push("sleep 600".to_string());
    }
    if let Some(code) = config.control.exit_code {
        lines.push(format!("exit {code}"));
    }
    std::fs::write(workspace.join(".agent.sh"), lines.join("\n") + "\n")?;
    let mut inner = Command::new("sh");
    inner.arg(".agent.sh").current_dir(&workspace);
    let deadline = config.budgets.hard_deadline_ms;
    let started = Instant::now();
    // Under the campaign's charges: the deadline clamped to the time left,
    // the elapsed bound checked before and after.
    let (status, stdout) = charged_run(
        contain(Some(&layout.private), &workspace, &inner),
        Duration::from_millis(deadline),
        charges,
    )?;
    let mut usage = TaskUsage {
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap(),
        ..TaskUsage::default()
    };
    match status {
        None => usage.elapsed_ms = usage.elapsed_ms.max(deadline),
        Some(status) if !status.success() => {
            return Err(RunError::ControlExited {
                task: entry.id.clone(),
                code: status.code(),
            });
        }
        Some(_) => {}
    }
    let private = layout.private.to_string_lossy();
    let mut calls = 0u32;
    let mut repository_access = Vec::new();
    let mut outputs = Vec::new();
    for line in stdout.lines() {
        match line.strip_prefix(TOOL_LINE) {
            Some(call) => {
                calls += 1;
                let climbs = call.split_whitespace().any(|arg| {
                    Path::new(arg)
                        .components()
                        .any(|c| c == Component::ParentDir)
                });
                if climbs || call.contains(&*private) {
                    repository_access.push(call.trim().to_string());
                }
            }
            None => outputs.push(line.to_string()),
        }
    }
    usage.tool_calls = calls;
    let hidden = if config.budgets.exhausted(&usage).is_some() {
        HiddenResults::new()
    } else {
        fresh_dir(&layout.tree)?;
        copy_tree(&prepared.snapshot, &layout.tree)?;
        overlay_patch(&workspace.join("patch"), &layout.tree)?;
        grade(&layout.tree, &prepared.hidden, layout, &entry.id, charges)?
    };
    let terminal = hidden_terminal(
        prepared.hidden.iter().map(|(name, _)| name.as_str()),
        &hidden,
        &usage,
        &config.budgets,
    );
    Ok(NoRepositoryControl {
        task: entry.id.clone(),
        entry_digest: prepared.audit.entry_digest.clone(),
        provider: provider.clone(),
        execution_image: config.settings.execution_image.clone(),
        analysis_family_digest: digest.to_string(),
        terminal,
        started: true,
        repository_access,
        future_answers: future_answers(entry, &outputs.join("\n")),
    })
}

pub fn run(config: &Config, host: Host) -> Result<Run, RunError> {
    let started_at_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let mut profile: RunProfile = super::campaign::profile(
        config.scale,
        128,
        config.elapsed_bound_ms,
        config.approval.clone(),
    );
    profile.name = format!("{}-anchor", profile.name);
    profile.budgets = config.budgets.clone();
    profile.envelope.processes = 2;
    profile.envelope.temp_roots = 4;
    profile.envelope.store_bytes = config.store_bound_bytes;
    profile.approved()?;
    let profile_digest = profile.digest()?;
    config.settings.validate()?;
    config.corpus.validate()?;
    let witness_value: Value =
        serde_json::from_slice(&std::fs::read(&config.witness)?).map_err(std::io::Error::other)?;
    parse_witness(&witness_value)?;
    let mut family = super::campaign::family(&profile);
    family.transfer_criterion = config.transfer_criterion.clone();
    let frozen =
        FrozenFamily::freeze(&family).map_err(|e| std::io::Error::other(format!("{e:?}")))?;
    let family_digest = frozen.analysis_family_digest.clone();
    let bound = config
        .settings
        .preparation_bound_ms
        .ok_or(SettingsRefused::NoPreparationBound)?;
    // Every proof and control runs inside the containment; a host without
    // it grades nothing, and says so instead of erroring every test.
    if !(host.namespaces)() {
        return Err(RunError::NoContainment);
    }
    let mut charges = Charges::new(profile.envelope.clone());
    prepare_publish(&config.publish, &[REPORT_FILE, MANIFEST_FILE]).map_err(publish_refused)?;
    let root = charges.occupy()?;
    let private = root.path().join("private");
    let layout = Layout {
        root: root.path().to_path_buf(),
        tasks: private.join("tasks"),
        tree: private.join("tree"),
        target: private.join("target"),
        private,
    };
    std::fs::create_dir_all(&layout.tasks)?;

    // The time study: prepare the first tasks, measure, project, and stop
    // for approval before the rest is paid for.
    let mut prepared: BTreeMap<String, Result<Prepared, AnchorTerminal>> = BTreeMap::new();
    let mut measured = Vec::new();
    let mut tasks = Vec::new();
    for entry in &config.corpus.entries {
        let started = Instant::now();
        let outcome = prepare(entry, host, &layout.tasks, &mut charges, &layout.root)?;
        let prepare_ms = u64::try_from(started.elapsed().as_millis()).unwrap();
        charges.store_bytes(&layout.root)?;
        charges.elapsed()?;
        if measured.len() < eval_core::TIME_STUDY_TASKS {
            measured.push(Preparation {
                task: entry.id.clone(),
                entry_digest: entry.digest()?,
                prepare_ms,
            });
        }
        prepared.insert(entry.id.clone(), outcome);
        tasks.push((entry, prepare_ms));
        if measured.len() == eval_core::TIME_STUDY_TASKS
            && prepared.len() == eval_core::TIME_STUDY_TASKS
            && let projected @ Affordability::StopForApproval { .. } =
                time_study(&config.corpus, &measured, bound)?
        {
            return Err(RunError::StopForApproval(projected));
        }
    }
    let affordability = time_study(&config.corpus, &measured, bound)?;

    let mut audits = BTreeMap::new();
    let mut proofs = BTreeMap::new();
    let mut controls_by_pair: BTreeMap<ProviderProfile, BTreeMap<String, ClassifiedControl>> =
        BTreeMap::new();
    let mut outcomes = Vec::new();
    for (entry, prepare_ms) in tasks {
        let mut outcome = TaskOutcome {
            id: entry.id.clone(),
            terminal: AnchorTerminal::Indeterminate,
            prepare_ms,
            audit: None,
            audit_refused: None,
            insufficiency: None,
            controls: Vec::new(),
            verdicts: Vec::new(),
        };
        let ready = match prepared.remove(&entry.id).unwrap() {
            Ok(ready) => ready,
            Err(terminal) => {
                outcome.terminal = terminal;
                outcomes.push(outcome);
                continue;
            }
        };
        if entry.family == Family::Django {
            outcome.terminal =
                AnchorTerminal::Unsupported(RealHistoryUnsupported::UnsupportedRuntime {
                    family: entry.family,
                });
            outcomes.push(outcome);
            continue;
        }
        outcome.audit = Some(ready.audit.clone());
        audits.insert(entry.id.clone(), ready.audit.clone());
        if let Err(refused) = ready.audit.validate_for(entry) {
            outcome.audit_refused = Some(refused);
            outcome.terminal = AnchorTerminal::Skipped(RealHistorySkip::MissingCutoffEvidence);
            outcomes.push(outcome);
            continue;
        }
        fresh_dir(&layout.tree)?;
        copy_tree(&ready.snapshot, &layout.tree)?;
        let hidden = grade(
            &layout.tree,
            &ready.hidden,
            &layout,
            &entry.id,
            &mut charges,
        )?;
        fresh_dir(&layout.tree)?;
        copy_tree(&ready.fix, &layout.tree)?;
        let reference = grade(
            &layout.tree,
            &ready.hidden,
            &layout,
            &entry.id,
            &mut charges,
        )?;
        // The fix tree is not left where the control's grade could read it.
        fresh_dir(&layout.tree)?;
        let proof = InsufficiencyProof {
            task: entry.id.clone(),
            entry_digest: ready.audit.entry_digest.clone(),
            snapshot_digest: ready.audit.snapshot_digest.clone(),
            hidden,
            reference,
        };
        let proven = proof.validate().is_ok();
        let judged = if proven {
            Terminal::Fail
        } else {
            Terminal::Indeterminate
        };
        outcome.terminal = AnchorTerminal::judged(judged);
        outcome.insufficiency = Some(proof.clone());
        proofs.insert(entry.id.clone(), proof);
        if proven {
            for provider in &config.settings.providers {
                let control = self::control(
                    &ControlRun {
                        entry,
                        prepared: &ready,
                        provider,
                        config,
                        digest: &family_digest,
                        layout: &layout,
                    },
                    &mut charges,
                )?;
                // No agent runs with the repository in this shell yet; the
                // proven current-tree-only run (`Fail`) is the repository-
                // bearing baseline the control is judged against.
                let expected = RepositoryComparison {
                    task: entry.id.clone(),
                    entry_digest: ready.audit.entry_digest.clone(),
                    provider: provider.clone(),
                    execution_image: config.settings.execution_image.clone(),
                    analysis_family_digest: family_digest.clone(),
                    terminal: judged,
                    started: true,
                };
                let classified = classify_control(&control, &expected).map_err(|refused| {
                    RunError::NotComparable {
                        task: entry.id.clone(),
                        refused,
                    }
                })?;
                controls_by_pair
                    .entry(provider.clone())
                    .or_default()
                    .insert(entry.id.clone(), classified.clone());
                outcome.controls.push(control);
                outcome.verdicts.push(classified);
            }
        }
        charges.store_bytes(&layout.root)?;
        outcomes.push(outcome);
    }
    let role = AnchorRole::Pilot;
    let mut accounting = Vec::new();
    let mut claims = Vec::new();
    for provider in &config.settings.providers {
        let classified = controls_by_pair.remove(provider).unwrap_or_default();
        let (set, pair) = anchor_set(
            &config.corpus,
            role,
            &audits,
            &proofs,
            &classified,
            provider,
        )?;
        claims.push(PairClaim {
            provider: provider.clone(),
            claim: family
                .claim_class(&frozen, WorldProvenance::RealHistory, Some(&set))
                .map_err(|e| std::io::Error::other(format!("{e:?}")))?,
        });
        accounting.push(pair);
    }

    let corpus_digest = config.corpus.digest()?;
    let run_identity = identity(
        &profile,
        SIMULATOR_VERSION,
        0,
        json!({
            "corpus": corpus_digest,
            "settings": sha256_hex(&serde_json::to_vec(&config.settings).unwrap()),
            "transfer_criterion": config.transfer_criterion,
            "control": config.control,
        }),
        &[std::env::current_exe().unwrap()],
    );
    charges.vacate(root)?;
    charges.retain_publish_root()?;
    let mut report = AnchorReport {
        schema: ANCHOR_REPORT_SCHEMA.to_string(),
        eval_run_id: eval_run_id(&run_identity).unwrap(),
        profile_digest,
        claim_boundary: ClaimBoundary::pinned(),
        corpus_digest,
        role,
        time_study: affordability,
        tasks: outcomes,
        accounting,
        claims,
        envelope: charges.envelope.clone(),
    };
    let report_bytes = charges.publish_bytes(|envelope| {
        report.envelope = envelope.clone();
        serde_json::to_vec_pretty(&serde_json::to_value(&report).unwrap()).unwrap()
    })?;
    let published: Value = serde_json::from_slice(&report_bytes).unwrap();
    let manifest = suite_c_manifest(ManifestInputs {
        identity: run_identity,
        eval_run_id: report.eval_run_id.clone(),
        sample: format!("anchor:{}", config.corpus.entries.len()),
        result_digest: protocol_digest("eval-anchor-result/v1", &published).unwrap(),
        witness_digest: protocol_digest(eval_core::WITNESS_DIGEST_PROTOCOL, &witness_value)
            .unwrap(),
        cut_receipts: vec![eval_core::CutReceipt {
            cut: eval_core::Cut::EndOfRun,
            outcome: eval_core::CutOutcome::Reached,
        }],
        execution_mode: eval_core::ExecutionMode::Generate,
        envelope: report.envelope.clone(),
        started_at_ms,
        task_corpus: format!("anchor:{}", report.corpus_digest),
        judge: suite_d::JUDGE_VERSION.to_string(),
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest.to_value()).unwrap();
    publish_file(&config.publish.join(REPORT_FILE), &report_bytes).map_err(publish_refused)?;
    publish_file(&config.publish.join(MANIFEST_FILE), &manifest_bytes).map_err(publish_refused)?;
    Ok(Run {
        report,
        report_bytes,
        manifest,
        manifest_bytes,
    })
}

fn publish_refused((path, kind): (PathBuf, std::io::ErrorKind)) -> RunError {
    RunError::Publish { path, kind }
}

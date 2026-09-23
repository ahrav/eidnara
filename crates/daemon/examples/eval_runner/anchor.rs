//! The real-history anchor shell: clones each anchor repository, builds the
//! cutoff snapshot, audits the cutoff, runs the current-tree-only
//! insufficiency proof, runs the no-repository control per provider inside
//! the Suite D containment, and publishes the per-provider accounting.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use context_core::canonical_json::protocol_digest;
use eval_core::{
    Affordability, AnchorCorpus, AnchorEntry, AnchorError, AnchorRole, Approval, ClaimBoundary,
    ClaimDerivation, ControlVerdict, CutoffAudit, CutoffRefused, Family, InsufficiencyProof,
    NoRepositoryControl, PairAccounting, Preparation, ProfileError, ProviderProfile,
    RealHistorySettings, RunProfile, Scale, SettingsRefused, SkipReason, Terminal,
    TimeStudyRefused, UnsupportedReason, WitnessError, WorldProvenance, anchor_set,
    classify_control, derive_claim_class, eval_run_id, future_answers, parse_witness, time_study,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::aging::{ManifestInputs, suite_c_manifest};
use super::campaign::{Charges, identity, prepare_publish, publish_file};
use super::suite_d::{self, contain, read_files, run_bounded, run_hidden};

pub const SIMULATOR_VERSION: &str = "eval-anchor-shell/v1";
pub const ANCHOR_REPORT_SCHEMA: &str = "eval-anchor-report/v1";
pub const REPORT_FILE: &str = "anchor-report.json";
pub const MANIFEST_FILE: &str = "manifest.json";
const TOOL_LINE: &str = "eval-anchor-tool";
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// What the runner fetched at run time for one entry and never persists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub issue_text: String,
    pub issue_created_ms: i64,
}

/// The host seams: how a repository is cloned and how an issue is fetched.
#[derive(Debug, Clone, Copy)]
pub struct Host {
    pub clone: fn(&AnchorEntry, &Path) -> std::io::Result<()>,
    pub fetch: fn(&AnchorEntry) -> Option<Fetched>,
}

/// What the scripted control agent does from the statement alone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ControlScript {
    /// Write the fix from memory: the memorized case.
    pub memorize: bool,
    /// Read the repository snapshot it was not given: seeded contamination.
    pub reach_repository: bool,
    /// Name the pull request in its output: seeded future knowledge.
    pub cite_future: bool,
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
    pub control: ControlScript,
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
    #[error("suite d shell: {0}")]
    SuiteD(#[from] suite_d::RunError),
    #[error("envelope exceeded: {0:?}")]
    Envelope(#[from] eval_core::EnvelopeExceeded),
    #[error("publish refused at {}: {kind}", path.display())]
    Publish {
        path: PathBuf,
        kind: std::io::ErrorKind,
    },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskOutcome {
    pub id: String,
    pub terminal: Terminal,
    #[serde(with = "eval_core::decimal")]
    pub prepare_ms: u64,
    pub audit: Option<CutoffAudit>,
    pub audit_refused: Option<CutoffRefused>,
    pub insufficiency: Option<InsufficiencyProof>,
    pub controls: BTreeMap<String, NoRepositoryControl>,
    pub verdicts: BTreeMap<String, ControlVerdict>,
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
    pub accounting: BTreeMap<String, PairAccounting>,
    pub claims: BTreeMap<String, ClaimDerivation>,
    pub envelope: eval_core::Envelope,
}

pub struct Run {
    pub report: AnchorReport,
    pub report_bytes: Vec<u8>,
    pub manifest: eval_core::Manifest,
    pub manifest_bytes: Vec<u8>,
}

fn git(dir: &Path, args: &[&str]) -> std::io::Result<Option<String>> {
    let mut command = Command::new("git");
    command.args(args).current_dir(dir).stderr(Stdio::null());
    Ok(run_bounded(command, GIT_TIMEOUT)
        .map_err(|e| std::io::Error::other(e.to_string()))?
        .filter(|(status, _)| status.success())
        .map(|(_, out)| out))
}

/// One prepared task: the clone, the snapshot at the base commit, the fix's
/// added paths and test files, and the audit inputs the repository holds.
struct Prepared {
    snapshot: PathBuf,
    audit: CutoffAudit,
    hidden: Vec<(String, String)>,
    fix_files: BTreeMap<String, String>,
    fetched: Fetched,
}

fn prepare(
    entry: &AnchorEntry,
    host: Host,
    root: &Path,
) -> Result<Result<Prepared, Terminal>, RunError> {
    let repo = root.join("repos").join(&entry.id);
    std::fs::create_dir_all(&repo)?;
    if (host.clone)(entry, &repo).is_err() {
        return Ok(Err(Terminal::Unsupported(
            UnsupportedReason::SourceUnavailable,
        )));
    }
    let Some(fetched) = (host.fetch)(entry) else {
        return Ok(Err(Terminal::Unsupported(
            UnsupportedReason::SourceUnavailable,
        )));
    };
    let committed = |sha: &str| -> std::io::Result<Option<i64>> {
        Ok(git(&repo, &["show", "-s", "--format=%ct", sha])?
            .and_then(|out| out.trim().parse::<i64>().ok())
            .map(|seconds| seconds * 1_000))
    };
    let (Some(base_ms), Some(fix_ms)) = (committed(&entry.base_sha)?, committed(&entry.fix_sha)?)
    else {
        return Ok(Err(Terminal::Unsupported(
            UnsupportedReason::SourceUnavailable,
        )));
    };
    let snapshot = root.join("snapshots").join(&entry.id);
    let _ = std::fs::remove_dir_all(&snapshot);
    std::fs::create_dir_all(&snapshot)?;
    let archive = Command::new("sh")
        .args([
            "-c",
            "git archive \"$1\" | tar -x -C \"$2\"",
            "sh",
            &entry.base_sha,
        ])
        .arg(&snapshot)
        .current_dir(&repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !archive.success() {
        return Ok(Err(Terminal::Unsupported(
            UnsupportedReason::SourceUnavailable,
        )));
    }
    let added = git(
        &repo,
        &[
            "diff",
            "--name-only",
            "--diff-filter=A",
            &entry.base_sha,
            &entry.fix_sha,
        ],
    )?
    .unwrap_or_default();
    let changed = git(
        &repo,
        &["diff", "--name-only", &entry.base_sha, &entry.fix_sha],
    )?
    .unwrap_or_default();
    let mut hidden = Vec::new();
    let mut fix_files = BTreeMap::new();
    for path in changed.lines().filter(|l| !l.is_empty()) {
        let content =
            git(&repo, &["show", &format!("{}:{path}", entry.fix_sha)])?.unwrap_or_default();
        match path
            .strip_prefix("tests/")
            .and_then(|p| p.strip_suffix(".rs"))
        {
            Some(name) if added.lines().any(|a| a == path) => {
                hidden.push((name.to_string(), content))
            }
            _ => {
                fix_files.insert(path.to_string(), content);
            }
        }
    }
    let files = read_files(&snapshot)?;
    let audit = CutoffAudit {
        task: entry.id.clone(),
        cutoff_ms: entry.cutoff_ms,
        base_committed_ms: base_ms,
        fix_committed_ms: fix_ms,
        issue_created_ms: fetched.issue_created_ms,
        snapshot_digest: protocol_digest(
            "eval-anchor-snapshot/v1",
            &serde_json::to_value(&files).unwrap(),
        )
        .unwrap(),
        fix_paths_present: added
            .lines()
            .any(|path| !path.is_empty() && files.contains_key(path)),
    };
    Ok(Ok(Prepared {
        snapshot,
        audit,
        hidden,
        fix_files,
        fetched,
    }))
}

/// The control: the statement alone in an otherwise empty workspace inside
/// the containment, then the patch it wrote graded on a fresh snapshot.
struct ControlRun<'a> {
    entry: &'a AnchorEntry,
    prepared: &'a Prepared,
    provider: &'a ProviderProfile,
    config: &'a Config,
    digest: &'a str,
    root: &'a Path,
    target: &'a Path,
}

fn control(run: &ControlRun<'_>, charges: &mut Charges) -> Result<NoRepositoryControl, RunError> {
    let ControlRun {
        entry,
        prepared,
        provider,
        config,
        digest,
        root,
        target,
    } = *run;
    let workspace = root.join("control");
    let _ = std::fs::remove_dir_all(&workspace);
    std::fs::create_dir_all(workspace.join("patch"))?;
    std::fs::write(workspace.join("STATEMENT.md"), &prepared.fetched.issue_text)?;
    let tool = |name: &str, argument: &str, command: &str| {
        format!("echo '{TOOL_LINE} {name} {argument}'\n{command}")
    };
    let mut lines = vec![
        "set -e".to_string(),
        tool("cat", "STATEMENT.md", "cat STATEMENT.md >/dev/null"),
    ];
    if config.control.memorize {
        suite_d::write_files(&workspace.join(".memory"), &prepared.fix_files)?;
        for path in prepared.fix_files.keys() {
            lines.push(tool(
                "write",
                &format!("patch/{path}"),
                &format!(
                    "mkdir -p \"$(dirname 'patch/{path}')\" && cp '.memory/{path}' 'patch/{path}'"
                ),
            ));
        }
        lines.push("rm -r .memory".to_string());
    }
    if config.control.reach_repository {
        let src = prepared.snapshot.join("src/lib.rs");
        lines.push(tool(
            "cat",
            &src.to_string_lossy(),
            &format!("cat '{}' || true", src.display()),
        ));
    }
    if config.control.cite_future {
        lines.push(format!(
            "echo 'this was fixed in #{}'",
            entry.pull_request.unwrap_or(0)
        ));
    }
    std::fs::write(workspace.join(".agent.sh"), lines.join("\n") + "\n")?;
    let mut inner = Command::new("sh");
    inner.arg(".agent.sh").current_dir(&workspace);
    let private = root.join("private");
    std::fs::create_dir_all(&private)?;
    charges.process_started()?;
    let output = run_bounded(
        contain(&private, &workspace, &inner),
        Duration::from_millis(config.settings_deadline()),
    );
    charges.process_ended();
    let stdout = output?.map(|(_, out)| out).unwrap_or_default();
    let repos = root.join("repos");
    let snapshots = root.join("snapshots");
    let mut repository_access = Vec::new();
    let mut outputs = Vec::new();
    for line in stdout.lines() {
        match line.strip_prefix(TOOL_LINE) {
            Some(call) => {
                if call.contains(&*repos.to_string_lossy())
                    || call.contains(&*snapshots.to_string_lossy())
                {
                    repository_access.push(call.trim().to_string());
                }
            }
            None => outputs.push(line.to_string()),
        }
    }
    let patch = read_files(&workspace.join("patch"))?;
    let graded = root.join("graded");
    let _ = std::fs::remove_dir_all(&graded);
    std::fs::create_dir_all(&graded)?;
    suite_d::write_files(&graded, &read_files(&prepared.snapshot)?)?;
    suite_d::write_files(&graded, &patch)?;
    let hidden = run_hidden(
        &graded,
        &prepared.hidden,
        target,
        Duration::from_millis(config.settings_deadline()),
        charges,
    )?;
    let terminal = if hidden.is_empty() {
        Terminal::Indeterminate
    } else if hidden
        .values()
        .all(|o| *o == eval_core::HiddenOutcome::Passed)
    {
        Terminal::Pass
    } else {
        Terminal::Fail
    };
    Ok(NoRepositoryControl {
        task: entry.id.clone(),
        provider: provider.clone(),
        execution_image: config.settings.execution_image.clone(),
        analysis_family_digest: digest.to_string(),
        terminal,
        repository_access,
        future_answers: future_answers(entry, &outputs.join("\n")),
    })
}

impl Config {
    fn settings_deadline(&self) -> u64 {
        suite_d::BUDGETS.hard_deadline_ms
    }
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
    profile.budgets = suite_d::BUDGETS;
    profile.envelope.processes = 2;
    profile.envelope.temp_roots = 4;
    profile.approved()?;
    let profile_digest = profile.digest()?;
    config.settings.validate()?;
    config.corpus.validate()?;
    let witness_value: Value =
        serde_json::from_slice(&std::fs::read(&config.witness)?).map_err(std::io::Error::other)?;
    parse_witness(&witness_value)?;
    let family = super::campaign::family(&profile);
    let family_digest = family
        .digest()
        .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
    let criterion = config.settings.transfer_criterion.as_ref();
    let mut charges = Charges::new(profile.envelope.clone());
    prepare_publish(&config.publish, &[REPORT_FILE, MANIFEST_FILE]).map_err(publish_refused)?;
    let root = charges.occupy()?;
    let target = root.path().join("target");
    let deadline = Duration::from_millis(config.settings_deadline());

    // The time study: prepare the first tasks, measure, project, and stop
    // for approval before the rest is paid for.
    let mut prepared: BTreeMap<String, Result<Prepared, Terminal>> = BTreeMap::new();
    let mut measured = Vec::new();
    let mut tasks = Vec::new();
    for entry in &config.corpus.entries {
        let started = Instant::now();
        let outcome = prepare(entry, host, root.path())?;
        let prepare_ms = u64::try_from(started.elapsed().as_millis()).unwrap();
        if measured.len() < eval_core::TIME_STUDY_TASKS {
            measured.push(Preparation {
                task: entry.id.clone(),
                prepare_ms,
            });
        }
        prepared.insert(entry.id.clone(), outcome);
        tasks.push((entry, prepare_ms));
        if measured.len() == eval_core::TIME_STUDY_TASKS
            && prepared.len() == eval_core::TIME_STUDY_TASKS
        {
            let bound = config.settings.preparation_bound_ms.unwrap();
            if let projected @ Affordability::StopForApproval { .. } =
                time_study(&config.corpus, &measured, bound)?
            {
                return Err(RunError::StopForApproval(projected));
            }
        }
    }
    let affordability = time_study(
        &config.corpus,
        &measured,
        config.settings.preparation_bound_ms.unwrap(),
    )?;

    let mut audits = BTreeMap::new();
    let mut proofs = BTreeMap::new();
    let mut controls_by_pair: BTreeMap<String, BTreeMap<String, ControlVerdict>> = BTreeMap::new();
    let mut outcomes = Vec::new();
    for (entry, prepare_ms) in tasks {
        let mut outcome = TaskOutcome {
            id: entry.id.clone(),
            terminal: Terminal::Indeterminate,
            prepare_ms,
            audit: None,
            audit_refused: None,
            insufficiency: None,
            controls: BTreeMap::new(),
            verdicts: BTreeMap::new(),
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
            outcome.terminal = Terminal::Unsupported(UnsupportedReason::UnsupportedRuntime {
                family: entry.family,
            });
            outcomes.push(outcome);
            continue;
        }
        outcome.audit = Some(ready.audit.clone());
        if let Err(refused) = ready.audit.validate() {
            outcome.audit_refused = Some(refused);
            outcome.terminal = Terminal::Skipped(SkipReason::MissingCutoffEvidence);
            audits.insert(entry.id.clone(), ready.audit.clone());
            outcomes.push(outcome);
            continue;
        }
        audits.insert(entry.id.clone(), ready.audit.clone());
        // The current-tree-only run: the snapshot, the hidden tests, no agent.
        let tree = root.path().join("tree");
        let _ = std::fs::remove_dir_all(&tree);
        std::fs::create_dir_all(&tree)?;
        suite_d::write_files(&tree, &read_files(&ready.snapshot)?)?;
        let proof = InsufficiencyProof {
            task: entry.id.clone(),
            hidden: run_hidden(&tree, &ready.hidden, &target, deadline, &mut charges)?,
        };
        outcome.terminal = match proof.validate() {
            Ok(()) => Terminal::Fail,
            Err(_) => Terminal::Indeterminate,
        };
        outcome.insufficiency = Some(proof.clone());
        proofs.insert(entry.id.clone(), proof);
        for provider in &config.settings.providers {
            let control = self::control(
                &ControlRun {
                    entry,
                    prepared: &ready,
                    provider,
                    config,
                    digest: &family_digest,
                    root: root.path(),
                    target: &target,
                },
                &mut charges,
            )?;
            let comparison = NoRepositoryControl {
                terminal: Terminal::Indeterminate,
                repository_access: Vec::new(),
                future_answers: Vec::new(),
                ..control.clone()
            };
            let verdict = classify_control(&control, &comparison)
                .expect("the control is built from its comparison");
            controls_by_pair
                .entry(provider.key())
                .or_default()
                .insert(entry.id.clone(), verdict.clone());
            outcome.controls.insert(provider.key(), control);
            outcome.verdicts.insert(provider.key(), verdict);
        }
        outcomes.push(outcome);
    }
    let role = AnchorRole::Pilot;
    let mut accounting = BTreeMap::new();
    let mut claims = BTreeMap::new();
    for provider in &config.settings.providers {
        let verdicts = controls_by_pair.remove(&provider.key()).unwrap_or_default();
        let (set, pair) = anchor_set(&config.corpus, role, &audits, &proofs, &verdicts, provider);
        claims.insert(
            provider.key(),
            derive_claim_class(WorldProvenance::RealHistory, Some(&set), criterion),
        );
        accounting.insert(provider.key(), pair);
    }

    let run_identity = identity(
        &profile,
        SIMULATOR_VERSION,
        0,
        json!({"corpus": config.corpus.digest(), "providers": config.settings.providers}),
        &std::env::current_exe().unwrap(),
    );
    charges.vacate(root)?;
    charges.retain_publish_root()?;
    let mut report = AnchorReport {
        schema: ANCHOR_REPORT_SCHEMA.to_string(),
        eval_run_id: eval_run_id(&run_identity).unwrap(),
        profile_digest,
        claim_boundary: ClaimBoundary::pinned(),
        corpus_digest: config.corpus.digest(),
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

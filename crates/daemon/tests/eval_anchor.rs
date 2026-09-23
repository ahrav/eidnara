#![cfg(all(unix, feature = "test-support"))]

//! The anchor shell against local real-history repositories: the time study,
//! the cutoff snapshot and audit, the current-tree-only insufficiency proof,
//! the no-repository control per provider, and the per-pair accounting.

mod support;

#[path = "../examples/eval_runner/aging.rs"]
#[allow(dead_code)]
mod aging;
#[path = "../examples/eval_runner/anchor.rs"]
#[allow(dead_code)]
mod anchor;
#[path = "../examples/eval_runner/campaign.rs"]
#[allow(dead_code)]
mod campaign;
#[path = "../examples/eval_runner/fault.rs"]
#[allow(dead_code)]
mod fault;
#[path = "../examples/eval_runner/shrink.rs"]
#[allow(dead_code)]
mod shrink;
#[path = "../examples/eval_runner/suite_d.rs"]
#[allow(dead_code)]
mod suite_d;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anchor::{Config, ControlScript, Fetched, Host, MANIFEST_FILE, REPORT_FILE, RunError};
use eval_core::{
    ANCHOR_CORPUS_SCHEMA, Affordability, AnchorCorpus, AnchorEntry, ClaimClass, Contamination,
    ControlVerdict, CutoffRefused, Family, HiddenOutcome, Oracle, ProviderProfile,
    RealHistorySettings, Scale, SkipReason, Terminal, TransferCriterion, UnmetClause,
    UnsupportedReason, generate_tasks,
};

const CUTOFF_SECONDS: i64 = 1_700_000_000;
const BASE_SECONDS: i64 = CUTOFF_SECONDS - 86_400;
const FIX_SECONDS: i64 = CUTOFF_SECONDS + 3_600;

fn git(dir: &Path, args: &[&str], seconds: i64) -> String {
    let stamp = format!("{seconds} +0000");
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_DATE", &stamp)
        .env("GIT_COMMITTER_DATE", &stamp)
        .env("GIT_AUTHOR_NAME", "eval")
        .env("GIT_AUTHOR_EMAIL", "eval@example.invalid")
        .env("GIT_COMMITTER_NAME", "eval")
        .env("GIT_COMMITTER_EMAIL", "eval@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// A real-history repository from a generated task: the base commit holds
/// the defect, the fix commit adds the correct body and the tests that catch
/// it. Returns the two SHAs.
fn history(dir: &Path, index: u32, fix_seconds: i64) -> (String, String) {
    let corpus = generate_tasks(0x5EED_D000_0000_0006, index + 1);
    let task = &corpus.tasks[index as usize];
    std::fs::create_dir_all(dir).unwrap();
    suite_d::write_files(dir, &task.files).unwrap();
    git(dir, &["init", "-q"], BASE_SECONDS);
    git(dir, &["add", "-A"], BASE_SECONDS);
    git(dir, &["commit", "-q", "-m", "base"], BASE_SECONDS);
    let base = git(dir, &["rev-parse", "HEAD"], BASE_SECONDS);
    suite_d::write_files(dir, &task.correct_fix).unwrap();
    for test in &task.hidden_tests {
        let path = dir.join(test.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, &test.content).unwrap();
    }
    git(dir, &["add", "-A"], fix_seconds);
    git(dir, &["commit", "-q", "-m", "fix"], fix_seconds);
    let fix = git(dir, &["rev-parse", "HEAD"], fix_seconds);
    (base, fix)
}

fn clone_local(entry: &AnchorEntry, into: &Path) -> std::io::Result<()> {
    let status = Command::new("git")
        .args(["clone", "-q", &entry.repository])
        .arg(into)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("clone failed"))
    }
}

fn fetch_issue(entry: &AnchorEntry) -> Option<Fetched> {
    (entry.issue != 404).then(|| Fetched {
        issue_text: format!(
            "`sum` is wrong for task {}; make it return the sum.",
            entry.id
        ),
        issue_created_ms: (CUTOFF_SECONDS - 7_200) * 1_000,
    })
}

const HOST: Host = Host {
    clone: clone_local,
    fetch: fetch_issue,
};

fn provider(model: &str) -> ProviderProfile {
    ProviderProfile {
        provider: "scripted".to_string(),
        model: model.to_string(),
        tokenizer_profile: "lexical".to_string(),
    }
}

fn settings(bound_ms: u64) -> RealHistorySettings {
    RealHistorySettings {
        providers: vec![provider("honest"), provider("memorizer")],
        execution_image: "in-process".to_string(),
        preparation_bound_ms: Some(bound_ms),
        transfer_criterion: Some(TransferCriterion {
            approved_by: "maintainer".to_string(),
            approved_at_run_id: "ab".repeat(32),
            min_valid_tasks: 5,
            required_families: BTreeSet::from(["cargo".to_string()]),
        }),
    }
}

fn witness(dir: &Path) -> PathBuf {
    let path = dir.join("witness").join(shrink::WITNESS_FILE);
    if !path.exists() {
        fn spawn_shrink(_: &shrink::ChildArgs) -> Command {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                "shrink_child_entrypoint_for_anchor",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ]);
            command
        }
        shrink::run(
            &shrink::Config {
                scale: Scale::S0,
                commits: 8,
                elapsed_bound_ms: 600_000,
                approval: Some(approval()),
                publish: dir.join("witness"),
                oracle: Oracle::RequiredCommits {
                    failing_at: 3,
                    slipping_at: 6,
                },
                replay_timeout: Duration::from_secs(120),
            },
            spawn_shrink,
        )
        .unwrap();
    }
    path
}

#[test]
#[ignore = "re-executed by the witness replays with their environment set"]
fn shrink_child_entrypoint_for_anchor() {
    if let Some(args) = shrink::ChildArgs::from_env() {
        shrink::child_main(&args);
    }
}

fn approval() -> eval_core::Approval {
    eval_core::Approval {
        approved_by: "maintainer".to_string(),
        approved_at_run_id: "ab".repeat(32),
    }
}

/// Five local Cargo-family repositories; `fix_seconds` moves one fix before
/// the cutoff and `missing_issue` makes one issue unfetchable.
fn corpus(dir: &Path, early_fix: Option<usize>, missing_issue: Option<usize>) -> AnchorCorpus {
    let entries = (0..5u32)
        .map(|index| {
            let repo = dir.join(format!("repo-{index}"));
            let fix_seconds = if early_fix == Some(index as usize) {
                CUTOFF_SECONDS - 60
            } else {
                FIX_SECONDS
            };
            let (base_sha, fix_sha) = history(&repo, index, fix_seconds);
            AnchorEntry {
                id: format!("cargo-{index}"),
                family: Family::Cargo,
                repository: repo.to_string_lossy().to_string(),
                license: "MIT".to_string(),
                base_sha,
                fix_sha,
                issue: if missing_issue == Some(index as usize) {
                    404
                } else {
                    100 + u64::from(index)
                },
                pull_request: Some(200 + u64::from(index)),
                cutoff_ms: CUTOFF_SECONDS * 1_000,
            }
        })
        .collect();
    AnchorCorpus {
        schema: ANCHOR_CORPUS_SCHEMA.to_string(),
        entries,
    }
}

fn config(dir: &Path, corpus: AnchorCorpus, control: ControlScript, bound_ms: u64) -> Config {
    Config {
        scale: Scale::S0,
        elapsed_bound_ms: 1_800_000,
        approval: Some(approval()),
        witness: witness(dir),
        publish: dir.join("out"),
        corpus,
        settings: settings(bound_ms),
        control,
    }
}

#[test]
fn every_anchor_task_has_an_audit_a_proof_and_a_control_and_the_pilot_never_transfers() {
    if !suite_d::namespaces_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), Some(3), Some(4));
    let config = config(
        dir.path(),
        corpus.clone(),
        ControlScript::default(),
        u64::MAX,
    );
    let run = anchor::run(&config, HOST).unwrap();
    let report = &run.report;
    assert!(matches!(
        report.time_study,
        Affordability::Affordable { .. }
    ));
    assert_eq!(report.tasks.len(), 5);
    for task in &report.tasks {
        assert!(
            task.prepare_ms > 0,
            "preparation time is measured, not asserted"
        );
    }
    let by_id = |id: &str| report.tasks.iter().find(|t| t.id == id).unwrap();
    for id in ["cargo-0", "cargo-1", "cargo-2"] {
        let task = by_id(id);
        assert_eq!(
            task.terminal,
            Terminal::Fail,
            "{id}: the tree alone is insufficient"
        );
        let audit = task.audit.as_ref().unwrap();
        audit.validate().unwrap();
        assert!(
            !audit.fix_paths_present,
            "the snapshot holds nothing the fix added"
        );
        assert_eq!(audit.snapshot_digest.len(), 64);
        let proof = task.insufficiency.as_ref().unwrap();
        proof.validate().unwrap();
        assert!(proof.hidden.values().any(|o| *o == HiddenOutcome::Failed));
        assert_eq!(task.controls.len(), 2, "one control per provider pair");
        for (pair, control) in &task.controls {
            assert_eq!(
                control.terminal,
                Terminal::Fail,
                "{pair}: the statement alone did not solve it"
            );
            assert!(control.repository_access.is_empty());
            assert_eq!(task.verdicts[pair], ControlVerdict::Eligible);
        }
    }
    let early = by_id("cargo-3");
    assert_eq!(
        early.terminal,
        Terminal::Skipped(SkipReason::MissingCutoffEvidence)
    );
    assert_eq!(early.audit_refused, Some(CutoffRefused::FixNotAfterCutoff));
    assert!(
        early.insufficiency.is_none(),
        "no run without cutoff evidence"
    );
    assert!(early.controls.is_empty());
    let missing = by_id("cargo-4");
    assert_eq!(
        missing.terminal,
        Terminal::Unsupported(UnsupportedReason::SourceUnavailable)
    );
    assert!(missing.audit.is_none());

    for (pair, accounting) in &report.accounting {
        assert_eq!(accounting.eligible.len(), 3, "{pair}");
        assert_eq!(
            accounting.cutoff_invalid["cargo-3"],
            CutoffRefused::FixNotAfterCutoff
        );
        assert_eq!(
            accounting.cutoff_invalid["cargo-4"],
            CutoffRefused::SnapshotDigestMissing,
            "an unfetchable source has no cutoff evidence at all"
        );
        let claim = &report.claims[pair];
        assert_eq!(claim.class, ClaimClass::GeneratedPhase1);
        assert!(
            claim.unmet.contains(&UnmetClause::AnchorSetIsPilot),
            "the pilot alone never establishes transfer"
        );
    }
    let published: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config.publish.join(REPORT_FILE)).unwrap()).unwrap();
    let text = published.to_string();
    assert!(
        !text.contains("make it return the sum"),
        "issue text is fetched, never published"
    );
    assert_eq!(published["corpus_digest"], corpus.digest());
    let corpus_text = serde_json::to_string(&corpus).unwrap();
    for entry in &corpus.entries {
        assert!(
            corpus_text.contains(&entry.base_sha),
            "commit SHAs are the corpus metadata"
        );
    }
    assert!(!corpus_text.contains("make it return the sum"));
    assert!(config.publish.join(MANIFEST_FILE).exists());
}

#[test]
fn a_memorizing_provider_is_excluded_for_its_pair_and_seeded_contamination_is_detected() {
    if !suite_d::namespaces_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), None, None);
    let memorized = config(
        dir.path(),
        corpus.clone(),
        ControlScript {
            memorize: true,
            ..ControlScript::default()
        },
        u64::MAX,
    );
    let run = anchor::run(&memorized, HOST).unwrap();
    for task in &run.report.tasks {
        for (pair, control) in &task.controls {
            assert_eq!(
                control.terminal,
                Terminal::Pass,
                "{pair}: solved from the statement alone"
            );
            assert_eq!(
                task.verdicts[pair],
                ControlVerdict::Excluded {
                    contamination: Contamination::Memorized
                }
            );
        }
    }
    for (pair, accounting) in &run.report.accounting {
        assert!(accounting.eligible.is_empty(), "{pair}");
        assert_eq!(
            accounting.excluded.len(),
            5,
            "{pair}: every task keeps its row and reason"
        );
        assert_eq!(run.report.claims[pair].skipped.len(), 5);
    }
    assert!(run.report.tasks.iter().all(|t| {
        t.verdicts
            .values()
            .all(|v| matches!(v, ControlVerdict::Excluded { .. }))
    }));

    let mut contaminated = config(
        dir.path(),
        corpus,
        ControlScript {
            reach_repository: true,
            cite_future: true,
            ..ControlScript::default()
        },
        u64::MAX,
    );
    contaminated.publish = dir.path().join("contaminated");
    let run = anchor::run(&contaminated, HOST).unwrap();
    for task in &run.report.tasks {
        for (pair, control) in &task.controls {
            assert!(
                !control.repository_access.is_empty(),
                "{pair}: the repository read was observed"
            );
            assert!(
                !control.future_answers.is_empty(),
                "{pair}: the pull request was named"
            );
            assert!(matches!(
                task.verdicts[pair],
                ControlVerdict::Excluded {
                    contamination: Contamination::RepositoryAccess { .. }
                }
            ));
        }
    }
}

#[test]
fn an_unaffordable_time_study_stops_for_approval_before_the_pilot_is_paid_for() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), None, None);
    let config = config(dir.path(), corpus, ControlScript::default(), 1);
    match anchor::run(&config, HOST) {
        Err(RunError::StopForApproval(Affordability::StopForApproval {
            projected_ms,
            bound_ms,
        })) => {
            assert!(projected_ms > bound_ms);
            assert_eq!(bound_ms, 1);
        }
        Err(other) => panic!("expected a stop for approval, got {other:?}"),
        Ok(_) => panic!("expected a stop for approval, got a run"),
    }
    assert!(
        !config.publish.join(REPORT_FILE).exists(),
        "nothing is published"
    );
}

#[test]
fn missing_settings_and_an_unaccepted_witness_refuse_before_execution() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), None, None);
    let mut config = config(dir.path(), corpus, ControlScript::default(), u64::MAX);
    config.settings.providers.clear();
    assert!(matches!(
        anchor::run(&config, HOST),
        Err(RunError::Settings(_))
    ));
    let mut no_bound = config.clone();
    no_bound.settings = settings(1);
    no_bound.settings.preparation_bound_ms = None;
    assert!(matches!(
        anchor::run(&no_bound, HOST),
        Err(RunError::Settings(_))
    ));
    let mut bad_witness = config.clone();
    bad_witness.settings = settings(1);
    bad_witness.witness = dir.path().join("nope.json");
    assert!(matches!(
        anchor::run(&bad_witness, HOST),
        Err(RunError::Io(_))
    ));
    assert!(!config.publish.exists());
}

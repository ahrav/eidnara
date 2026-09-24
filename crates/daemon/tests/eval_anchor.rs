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
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anchor::{
    AnchorTerminal, Config, ControlScript, Fetched, Host, MANIFEST_FILE, REPORT_FILE, RunError,
};
use eval_core::{
    ANCHOR_CORPUS_SCHEMA, Affordability, AnchorCorpus, AnchorEntry, AnchorError, CensorReason,
    ClaimClass, Contamination, ControlVerdict, CutoffRefused, Family, HiddenOutcome,
    InsufficiencyRefused, Oracle, ProviderProfile, RealHistorySettings, RealHistorySkip,
    RealHistoryUnsupported, Resource, Scale, TaskBudgets, Terminal, TransferCriterion, UnmetClause,
    generate_tasks,
};

const CUTOFF_SECONDS: i64 = 1_700_000_000;
const BASE_SECONDS: i64 = CUTOFF_SECONDS - 86_400;
const FIX_SECONDS: i64 = CUTOFF_SECONDS + 3_600;
const BLOB: [u8; 4] = [0xff, 0xfe, 0x00, 0x01];
const ASSETS_TEST: &str = r#"
#[test]
fn base_assets_survive_the_snapshot() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(std::fs::read(root.join("assets/blob.bin")).unwrap(), [0xff, 0xfe, 0x00, 0x01]);
    let link = std::fs::symlink_metadata(root.join("assets/link")).unwrap();
    assert!(link.file_type().is_symlink());
    let mode = std::fs::metadata(root.join("scripts/anchor-tool.sh")).unwrap().permissions().mode();
    assert_ne!(mode & 0o111, 0);
}
"#;

const SUPPORT_TEST: &str = r#"
#[test]
fn the_fixture_the_fix_added_is_beside_the_test() {
    assert_eq!(include_str!("fixture.txt"), "included");
}
"#;

/// How one fixture repository departs from a plain real-history task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    Plain,
    EarlyFix,
    MissingIssue,
    UnresolvableDependency,
    NestedTestFile,
    /// The base commit carries a build script that fails; the fix deletes
    /// it, so only the exact fix tree builds.
    DeletesBuildScript,
    /// The base commit is two commits behind the fix: an intervening commit
    /// after the cutoff adds a test file that is not the fix's.
    IntermediateCommit,
    /// The tree carries a megabyte that git stores in a few bytes, so every
    /// extracted tree is large and the clone is not.
    BulkyTree,
    /// The fix adds `tests/planted.rs` as a symlink to a file outside the
    /// repository.
    SymlinkedHiddenTest,
    /// The fix adds `tests/fixture.txt`, which every hidden test includes,
    /// and marks `tests/` `export-ignore`.
    TestSupportFile,
    /// The base commit carries `tests/fixture.txt` with other contents; the
    /// fix edits it, and the only hidden test reads it and nothing else.
    ModifiedTestSupport,
}

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
/// the defect and a binary file, a symlink, and an executable script; the
/// fix commit adds the correct body and the tests that catch the defect.
/// Returns the two SHAs.
fn history(dir: &Path, index: u32, variant: Variant) -> (String, String) {
    let corpus = generate_tasks(
        0x5EED_D000_0000_0006,
        std::num::NonZeroU32::new(index + 1).unwrap(),
    );
    let task = &corpus.tasks[index as usize];
    std::fs::create_dir_all(dir).unwrap();
    suite_d::write_files(dir, &task.files).unwrap();
    if variant == Variant::UnresolvableDependency {
        let manifest = dir.join("Cargo.toml");
        let mut text = std::fs::read_to_string(&manifest).unwrap();
        text.push_str("eidnara-absent-offline = \"1\"\n");
        std::fs::write(&manifest, text).unwrap();
    }
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    if variant == Variant::BulkyTree {
        std::fs::write(dir.join("assets/bulk.bin"), vec![0u8; 1 << 20]).unwrap();
    }
    if variant == Variant::ModifiedTestSupport {
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::write(dir.join("tests/fixture.txt"), "before the fix").unwrap();
    }
    if variant == Variant::DeletesBuildScript {
        std::fs::write(
            dir.join("build.rs"),
            "fn main() { panic!(\"the base tree does not build\"); }\n",
        )
        .unwrap();
    }
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets/blob.bin"), BLOB).unwrap();
    std::os::unix::fs::symlink("blob.bin", dir.join("assets/link")).unwrap();
    let tool = dir.join("scripts/anchor-tool.sh");
    std::fs::create_dir_all(tool.parent().unwrap()).unwrap();
    std::fs::write(&tool, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(dir, &["init", "-q"], BASE_SECONDS);
    git(dir, &["add", "-A"], BASE_SECONDS);
    git(dir, &["commit", "-q", "-m", "base"], BASE_SECONDS);
    let base = git(dir, &["rev-parse", "HEAD"], BASE_SECONDS);
    if variant == Variant::IntermediateCommit {
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::write(
            dir.join("tests/unrelated.rs"),
            "#[test]\nfn unrelated() {}\n",
        )
        .unwrap();
        // After the cutoff, as every fix-side commit must be.
        git(dir, &["add", "-A"], FIX_SECONDS - 1_800);
        git(
            dir,
            &["commit", "-q", "-m", "unrelated"],
            FIX_SECONDS - 1_800,
        );
    }
    if variant == Variant::DeletesBuildScript {
        std::fs::remove_file(dir.join("build.rs")).unwrap();
    }
    suite_d::write_files(dir, &task.correct_fix).unwrap();
    for test in &task.hidden_tests {
        let path = dir.join(test.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let content = match variant {
            Variant::TestSupportFile => format!("{}{ASSETS_TEST}{SUPPORT_TEST}", test.content),
            // The test judges nothing but its fixture.
            Variant::ModifiedTestSupport => SUPPORT_TEST.to_string(),
            _ => format!("{}{ASSETS_TEST}", test.content),
        };
        std::fs::write(path, content).unwrap();
    }
    if matches!(
        variant,
        Variant::TestSupportFile | Variant::ModifiedTestSupport
    ) {
        std::fs::write(dir.join("tests/fixture.txt"), "included").unwrap();
    }
    if variant == Variant::TestSupportFile {
        std::fs::write(dir.join(".gitattributes"), "tests/ export-ignore\n").unwrap();
    }
    if variant == Variant::NestedTestFile {
        let helper = dir.join("tests/nested/helper.rs");
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        std::fs::write(helper, "pub fn helper() {}\n").unwrap();
    }
    if variant == Variant::SymlinkedHiddenTest {
        std::os::unix::fs::symlink("/etc/hostname", dir.join("tests/planted.rs")).unwrap();
    }
    let fix_seconds = if variant == Variant::EarlyFix {
        CUTOFF_SECONDS - 60
    } else {
        FIX_SECONDS
    };
    git(dir, &["add", "-A"], fix_seconds);
    git(dir, &["commit", "-q", "-m", "fix"], fix_seconds);
    let fix = git(dir, &["rev-parse", "HEAD"], fix_seconds);
    (base, fix)
}

/// The corpus persists an `https://` clone URL; the local host resolves it
/// to the fixture directory it names under `LOCAL_HOST`.
const LOCAL_HOST: &str = "https://local.invalid";

fn clone_local(entry: &AnchorEntry, into: &Path, _deadline: Duration) -> std::io::Result<()> {
    let local = entry
        .repository
        .strip_prefix(LOCAL_HOST)
        .expect("a fixture URL");
    let status = Command::new("git")
        .args(["clone", "-q", "--", local])
        .arg(into)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("clone failed"))
    }
}

fn fetch_issue(entry: &AnchorEntry, _deadline: Duration) -> Option<Fetched> {
    (entry.issue != 404).then(|| Fetched {
        issue_text: format!(
            "`sum` is wrong for task {}; make it return the sum.",
            entry.id
        ),
        issue_created_ms: (CUTOFF_SECONDS - 7_200) * 1_000,
        issue_text_ms: (CUTOFF_SECONDS - 7_200) * 1_000,
        // Opened after the cutoff and before the fix landed.
        pull_request_created_ms: Some((CUTOFF_SECONDS + 1_800) * 1_000),
    })
}

const HOST: Host = Host {
    clone: clone_local,
    fetch: fetch_issue,
    namespaces: suite_d::namespaces_available,
};

/// For runs that refuse before any tree is graded: preparation never enters
/// the containment, so these tests hold on a host without namespaces too.
const PREPARING_HOST: Host = Host {
    namespaces: || true,
    ..HOST
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
    }
}

fn criterion() -> TransferCriterion {
    TransferCriterion {
        approved_by: "maintainer".to_string(),
        approved_at_run_id: "ab".repeat(32),
        min_valid_tasks: 5,
        required_families: BTreeSet::from(["cargo".to_string()]),
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

/// One local Cargo-family repository per variant, in order.
/// The pilot composition over local repositories: eight Cargo, eight Tokio,
/// and four Django rows, in that order, one repository each. The first rows
/// take `variants`; the rest are early fixes, prepared but never graded, so
/// a test pays for the tasks it looks at. Every row is a Cargo crate; the
/// family is the corpus's label for it.
fn corpus(dir: &Path, variants: &[Variant]) -> AnchorCorpus {
    let pilot: Vec<Family> = eval_core::PILOT_COMPOSITION
        .iter()
        .flat_map(|(family, count)| std::iter::repeat_n(*family, *count as usize))
        .collect();
    let entries = pilot
        .iter()
        .enumerate()
        .map(|(index, &family)| {
            let variant = variants.get(index).copied().unwrap_or(Variant::EarlyFix);
            let index = u32::try_from(index).unwrap();
            let repo = dir.join(format!("repo-{index}"));
            let (base_sha, fix_sha) = history(&repo, index, variant);
            AnchorEntry {
                id: format!("{}-{index}", family.label()),
                family,
                repository: format!("{LOCAL_HOST}{}", repo.display()),
                license: "MIT".to_string(),
                base_sha,
                fix_sha,
                issue: if variant == Variant::MissingIssue {
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

const PLAIN: [Variant; 5] = [Variant::Plain; 5];

/// The verdict recorded for `control`'s provider pair on `task`.
fn verdict(task: &anchor::TaskOutcome, control: &eval_core::NoRepositoryControl) -> ControlVerdict {
    task.verdicts
        .iter()
        .find(|v| v.provider == control.provider)
        .unwrap()
        .verdict
        .clone()
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
        transfer_criterion: Some(criterion()),
        control,
        budgets: suite_d::BUDGETS,
        store_bound_bytes: 1 << 30,
    }
}

#[test]
fn every_anchor_task_has_an_audit_a_proof_and_a_control_and_the_pilot_never_transfers() {
    if !suite_d::namespaces_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(
        dir.path(),
        &[
            Variant::Plain,
            Variant::Plain,
            Variant::NestedTestFile,
            Variant::EarlyFix,
            Variant::MissingIssue,
            Variant::UnresolvableDependency,
        ],
    );
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
    assert_eq!(report.tasks.len(), 20, "the pilot composition, no more");
    for task in &report.tasks {
        assert!(
            task.prepare_ms > 0,
            "preparation time is measured, not asserted"
        );
    }
    for task in report.tasks.iter().filter(|t| t.id.starts_with("django-")) {
        assert_eq!(
            task.terminal,
            AnchorTerminal::Unsupported(RealHistoryUnsupported::UnsupportedRuntime {
                family: Family::Django
            })
        );
        assert!(task.audit.is_none() && task.insufficiency.is_none());
    }
    let by_id = |id: &str| report.tasks.iter().find(|t| t.id == id).unwrap();
    for id in ["cargo-0", "cargo-1", "cargo-2"] {
        let task = by_id(id);
        assert_eq!(
            task.terminal,
            AnchorTerminal::Fail,
            "{id}: the tree alone is insufficient"
        );
        let audit = task.audit.as_ref().unwrap();
        audit
            .validate_for(corpus.entries.iter().find(|e| e.id == id).unwrap())
            .unwrap();
        assert_eq!(audit.snapshot_digest.len(), 64);
        assert_ne!(
            audit.fix_tree_digest, audit.fix_parent_tree_digest,
            "the fix commit's tree is read next to its parent's"
        );
        let proof = task.insufficiency.as_ref().unwrap();
        proof.validate().unwrap();
        assert!(proof.hidden.values().any(|o| *o == HiddenOutcome::Failed));
        assert!(
            proof
                .reference
                .values()
                .all(|o| *o == HiddenOutcome::Passed),
            "{id}: the fix tree passes every hidden test, assets included"
        );
        assert_eq!(task.controls.len(), 2, "one control per provider pair");
        for control in &task.controls {
            let pair = &control.provider.model;
            assert_eq!(
                control.terminal,
                Terminal::Fail,
                "{pair}: the statement alone did not solve it"
            );
            assert!(control.repository_access.is_empty());
            assert_eq!(verdict(task, control), ControlVerdict::Eligible);
        }
    }
    let nested = by_id("cargo-2").insufficiency.as_ref().unwrap();
    assert_eq!(
        nested.hidden.len(),
        2,
        "a module under a tests/ subdirectory is not a hidden test target"
    );
    let early = by_id("cargo-3");
    assert_eq!(
        early.terminal,
        AnchorTerminal::Skipped(RealHistorySkip::MissingCutoffEvidence)
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
        AnchorTerminal::Unsupported(RealHistoryUnsupported::SourceUnavailable)
    );
    assert!(missing.audit.is_none());
    let unbuildable = by_id("cargo-5");
    assert_eq!(
        unbuildable.terminal,
        AnchorTerminal::Indeterminate,
        "a tree the runner cannot build is not proven insufficient"
    );
    let proof = unbuildable.insufficiency.as_ref().unwrap();
    assert!(proof.hidden.values().all(|o| *o == HiddenOutcome::Errored));
    assert_eq!(
        proof.validate(),
        Err(InsufficiencyRefused::ReferenceDoesNotPass)
    );
    assert!(
        unbuildable.controls.is_empty() && unbuildable.verdicts.is_empty(),
        "no control runs for a task without a proof"
    );

    assert_eq!(report.accounting.len(), 2);
    for accounting in &report.accounting {
        let pair = &accounting.provider.model;
        assert_eq!(
            accounting.eligible,
            BTreeSet::from(["cargo-0".into(), "cargo-1".into(), "cargo-2".into()]),
            "{pair}"
        );
        assert_eq!(
            accounting.cutoff_invalid["cargo-3"],
            CutoffRefused::FixNotAfterCutoff
        );
        assert!(
            accounting.cutoff_missing.contains("cargo-4"),
            "an unfetchable source has no cutoff evidence at all"
        );
        assert!(
            accounting.cutoff_missing.contains("django-16"),
            "an unsupported runtime is missing evidence, not a failed cutoff"
        );
        assert_eq!(
            accounting.cutoff_invalid["tokio-8"],
            CutoffRefused::FixNotAfterCutoff
        );
        assert!(accounting.insufficiency_refused.contains_key("cargo-5"));
        let claim = &report
            .claims
            .iter()
            .find(|c| c.provider == accounting.provider)
            .unwrap()
            .claim;
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
    assert_eq!(published["corpus_digest"], corpus.digest().unwrap());
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
    let corpus = corpus(dir.path(), &PLAIN);
    let memorized = config(
        dir.path(),
        corpus.clone(),
        ControlScript {
            memorize: true,
            ..ControlScript::default()
        },
        u64::MAX,
    );
    let first = anchor::run(&memorized, HOST).unwrap();
    for task in &first.report.tasks {
        for control in &task.controls {
            let pair = &control.provider.model;
            assert_eq!(
                control.terminal,
                Terminal::Pass,
                "{pair}: solved from the statement alone, every test in the file passing"
            );
            assert_eq!(
                verdict(task, control),
                ControlVerdict::Excluded {
                    contamination: Contamination::Memorized
                }
            );
        }
    }
    for (accounting, claim) in first.report.accounting.iter().zip(&first.report.claims) {
        let pair = &accounting.provider.model;
        assert_eq!(accounting.provider, claim.provider);
        assert!(accounting.eligible.is_empty(), "{pair}");
        assert_eq!(
            accounting.excluded.len(),
            5,
            "{pair}: every task keeps its row and reason"
        );
        assert_eq!(
            claim.claim.skipped.len(),
            20,
            "{pair}: no task in the pilot is valid for a memorizing pair"
        );
    }
    assert!(first.report.tasks.iter().all(|t| {
        t.verdicts
            .iter()
            .all(|v| matches!(v.verdict, ControlVerdict::Excluded { .. }))
    }));

    let mut contaminated = config(
        dir.path(),
        corpus,
        ControlScript {
            memorize: true,
            reach_repository: true,
            cite_future: true,
            ..ControlScript::default()
        },
        u64::MAX,
    );
    contaminated.publish = dir.path().join("contaminated");
    contaminated.transfer_criterion = None;
    let second = anchor::run(&contaminated, HOST).unwrap();
    for task in &second.report.tasks {
        for control in &task.controls {
            let pair = &control.provider.model;
            assert!(
                !control.repository_access.is_empty(),
                "{pair}: the relative repository read was observed"
            );
            assert_eq!(
                control.terminal,
                Terminal::Pass,
                "{pair}: the read was denied, so the base file never replaced the memorized fix"
            );
            assert!(
                !control.future_answers.is_empty(),
                "{pair}: the pull request was named"
            );
            assert!(matches!(
                verdict(task, control),
                ControlVerdict::Excluded {
                    contamination: Contamination::RepositoryAccess { .. }
                }
            ));
        }
    }
    assert_ne!(
        first.report.eval_run_id, second.report.eval_run_id,
        "the control script and settings are part of the run's identity"
    );
    let digest = |run: &anchor::Run| {
        run.report.tasks[0].controls[0]
            .analysis_family_digest
            .clone()
    };
    assert_ne!(
        digest(&first),
        digest(&second),
        "the transfer criterion is frozen into the analysis family"
    );
}

#[test]
fn a_control_past_its_deadline_is_censored_with_its_trace_and_a_failed_control_refuses() {
    if !suite_d::namespaces_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), &PLAIN);
    let mut stalled = config(
        dir.path(),
        corpus.clone(),
        ControlScript {
            reach_repository: true,
            stall: true,
            ..ControlScript::default()
        },
        u64::MAX,
    );
    stalled.settings.providers.truncate(1);
    stalled.budgets = TaskBudgets {
        hard_deadline_ms: 1_500,
        ..suite_d::BUDGETS
    };
    let run = anchor::run(&stalled, HOST).unwrap();
    for task in &run.report.tasks {
        for control in &task.controls {
            let pair = &control.provider.model;
            assert_eq!(
                control.terminal,
                Terminal::Censored {
                    reason: CensorReason::HardDeadlineMs
                },
                "{pair}"
            );
            assert!(
                !control.repository_access.is_empty(),
                "{pair}: the read announced before the deadline is kept"
            );
            assert!(matches!(
                verdict(task, control),
                ControlVerdict::Excluded {
                    contamination: Contamination::RepositoryAccess { .. }
                }
            ));
        }
    }

    let mut exited = config(
        dir.path(),
        corpus,
        ControlScript {
            exit_code: Some(3),
            ..ControlScript::default()
        },
        u64::MAX,
    );
    exited.publish = dir.path().join("exited");
    match anchor::run(&exited, HOST) {
        Err(RunError::ControlExited { task, code }) => {
            assert_eq!(task, "cargo-0");
            assert_eq!(code, Some(3));
        }
        Err(other) => panic!("expected the failed control to refuse, got {other:?}"),
        Ok(_) => panic!("expected the failed control to refuse, got a run"),
    }
    assert!(!exited.publish.join(REPORT_FILE).exists());
}

#[test]
fn an_unaffordable_time_study_stops_for_approval_before_the_pilot_is_paid_for() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), &PLAIN);
    let config = config(dir.path(), corpus, ControlScript::default(), 1);
    match anchor::run(&config, PREPARING_HOST) {
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
fn the_store_is_charged_while_preparing_not_after_the_pilot() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), &PLAIN);
    let mut config = config(dir.path(), corpus, ControlScript::default(), 1);
    config.store_bound_bytes = 1;
    match anchor::run(&config, PREPARING_HOST) {
        Err(RunError::Envelope(exceeded)) => {
            assert_eq!(exceeded.resource, Resource::StoreBytes);
            assert_eq!(exceeded.bound, 1);
        }
        Err(other) => panic!("expected the store bound during preparation, got {other:?}"),
        Ok(_) => panic!("expected the store bound during preparation, got a run"),
    }
    assert!(!config.publish.join(REPORT_FILE).exists());
}

#[test]
fn the_clone_is_charged_before_it_is_removed() {
    static CLONES: AtomicUsize = AtomicUsize::new(0);
    fn bulky_clone(entry: &AnchorEntry, into: &Path, deadline: Duration) -> std::io::Result<()> {
        CLONES.fetch_add(1, Ordering::SeqCst);
        clone_local(entry, into, deadline)?;
        // Untracked, so no snapshot or fix tree holds it: only the clone does.
        std::fs::write(into.join("bulk.bin"), vec![0u8; 4 << 20])
    }
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), &PLAIN);
    let mut config = config(dir.path(), corpus, ControlScript::default(), u64::MAX);
    config.store_bound_bytes = 2 << 20;
    let host = Host {
        clone: bulky_clone,
        ..PREPARING_HOST
    };
    match anchor::run(&config, host) {
        Err(RunError::Envelope(exceeded)) => {
            assert_eq!(exceeded.resource, Resource::StoreBytes);
        }
        Err(other) => panic!("expected the clone to trip the store bound, got {other:?}"),
        Ok(_) => panic!("expected the clone to trip the store bound, got a run"),
    }
    assert_eq!(
        CLONES.load(Ordering::SeqCst),
        1,
        "the first clone trips the bound while it exists; a charge after its removal would let every clone run"
    );
    assert!(!config.publish.join(REPORT_FILE).exists());
}

#[test]
fn every_extracted_tree_is_charged_while_the_clone_still_exists() {
    static CLONES: AtomicUsize = AtomicUsize::new(0);
    fn counted_clone(entry: &AnchorEntry, into: &Path, deadline: Duration) -> std::io::Result<()> {
        CLONES.fetch_add(1, Ordering::SeqCst);
        clone_local(entry, into, deadline)
    }
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), &[Variant::BulkyTree; 5]);
    let mut config = config(dir.path(), corpus, ControlScript::default(), u64::MAX);
    // Room for the clone and two extracted trees, not for the clone, the
    // snapshot, the fix tree, and the parent's scratch tree at once.
    config.store_bound_bytes = 7 << 19;
    let host = Host {
        clone: counted_clone,
        ..PREPARING_HOST
    };
    match anchor::run(&config, host) {
        Err(RunError::Envelope(exceeded)) => {
            assert_eq!(exceeded.resource, Resource::StoreBytes);
        }
        Err(other) => panic!("expected the extraction to trip the store bound, got {other:?}"),
        Ok(_) => panic!("expected the extraction to trip the store bound, got a run"),
    }
    assert_eq!(
        CLONES.load(Ordering::SeqCst),
        1,
        "the first task's trees trip the bound while its clone exists; a charge after the clone and scratch are gone sees two tasks' trees first"
    );
}

#[test]
fn the_elapsed_bound_is_charged_while_preparing() {
    static CLONES: AtomicUsize = AtomicUsize::new(0);
    fn counted_clone(entry: &AnchorEntry, into: &Path, deadline: Duration) -> std::io::Result<()> {
        CLONES.fetch_add(1, Ordering::SeqCst);
        clone_local(entry, into, deadline)
    }
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), &PLAIN);
    let mut config = config(dir.path(), corpus, ControlScript::default(), u64::MAX);
    config.elapsed_bound_ms = 1;
    let host = Host {
        clone: counted_clone,
        ..PREPARING_HOST
    };
    match anchor::run(&config, host) {
        Err(RunError::Envelope(exceeded)) => {
            assert_eq!(exceeded.resource, Resource::ElapsedMs);
        }
        Err(other) => panic!("expected the elapsed bound during preparation, got {other:?}"),
        Ok(_) => panic!("expected the elapsed bound during preparation, got a run"),
    }
    assert!(
        CLONES.load(Ordering::SeqCst) <= 1,
        "an exhausted campaign clock stops preparation at the next task, not after every clone"
    );
    assert!(!config.publish.join(REPORT_FILE).exists());
}

#[test]
fn the_fix_is_its_own_diff_and_its_whole_tree() {
    if !suite_d::namespaces_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(
        dir.path(),
        &[
            Variant::DeletesBuildScript,
            Variant::IntermediateCommit,
            Variant::SymlinkedHiddenTest,
            Variant::TestSupportFile,
            Variant::ModifiedTestSupport,
        ],
    );
    let mut config = config(dir.path(), corpus, ControlScript::default(), u64::MAX);
    config.settings.providers.truncate(1);
    let run = anchor::run(&config, HOST).unwrap();
    let by_id = |id: &str| run.report.tasks.iter().find(|t| t.id == id).unwrap();
    let deletes = by_id("cargo-0");
    let proof = deletes.insufficiency.as_ref().unwrap();
    assert!(
        proof.hidden.values().all(|o| *o == HiddenOutcome::Errored),
        "the base tree's build script fails every test"
    );
    assert!(
        proof
            .reference
            .values()
            .all(|o| *o == HiddenOutcome::Passed),
        "the reference is the fix commit's tree, the deleted build script gone"
    );
    assert_eq!(deletes.terminal, AnchorTerminal::Fail);
    let intermediate = by_id("cargo-1");
    let proof = intermediate.insufficiency.as_ref().unwrap();
    assert_eq!(
        proof.hidden.len(),
        2,
        "a test file an intervening commit added is not one of the fix's hidden tests"
    );
    assert!(!proof.hidden.contains_key("unrelated"));
    assert_eq!(intermediate.terminal, AnchorTerminal::Fail);
    let planted = by_id("cargo-2");
    let proof = planted.insufficiency.as_ref().unwrap();
    assert!(
        !proof.hidden.contains_key("planted"),
        "a symlink at a fix-added tests/ path is not a hidden test and is never read through"
    );
    assert_eq!(proof.hidden.len(), 2);
    let supported = by_id("cargo-3");
    let proof = supported.insufficiency.as_ref().unwrap();
    assert!(
        proof.hidden.values().all(|o| *o == HiddenOutcome::Failed),
        "the fixture the fix added is beside the test on the base tree too, so the test fails on the defect rather than erroring for want of its input: {:?}",
        proof.hidden
    );
    assert!(
        proof
            .reference
            .values()
            .all(|o| *o == HiddenOutcome::Passed)
    );
    assert_eq!(
        supported.terminal,
        AnchorTerminal::Fail,
        "a tests/ directory marked export-ignore is still in the snapshot and the fix tree"
    );
    let edited = by_id("cargo-4");
    let proof = edited.insufficiency.as_ref().unwrap();
    assert_eq!(
        proof.validate(),
        Err(InsufficiencyRefused::TreeAlreadyPasses),
        "a test that reads only a fixture the fix edited sees the fix's fixture on the base tree too, so it proves no defect: {:?}",
        proof.hidden
    );
    assert_eq!(edited.terminal, AnchorTerminal::Indeterminate);
}

#[test]
fn the_host_seams_are_given_the_campaign_time_left_and_a_named_pull_request_needs_its_time() {
    static DEADLINES: AtomicUsize = AtomicUsize::new(0);
    fn timed_clone(entry: &AnchorEntry, into: &Path, deadline: Duration) -> std::io::Result<()> {
        assert!(
            deadline <= Duration::from_millis(90_000),
            "the clone is given the campaign time left, got {deadline:?}"
        );
        DEADLINES.fetch_add(1, Ordering::SeqCst);
        clone_local(entry, into, deadline)
    }
    /// The first row's pull request has no creation time; the others do.
    fn no_pull_request_time(entry: &AnchorEntry, deadline: Duration) -> Option<Fetched> {
        assert!(deadline <= Duration::from_millis(90_000));
        let fetched = fetch_issue(entry, deadline)?;
        Some(Fetched {
            pull_request_created_ms: fetched
                .pull_request_created_ms
                .filter(|_| entry.issue != 100),
            ..fetched
        })
    }
    let dir = tempfile::tempdir().unwrap();
    // Every row an early fix: prepared, never graded.
    let corpus = corpus(dir.path(), &[]);
    let mut config = config(dir.path(), corpus, ControlScript::default(), u64::MAX);
    config.elapsed_bound_ms = 90_000;
    let host = Host {
        clone: timed_clone,
        fetch: no_pull_request_time,
        ..PREPARING_HOST
    };
    let run = anchor::run(&config, host).unwrap();
    assert_eq!(DEADLINES.load(Ordering::SeqCst), 20);
    let by_id = |id: &str| run.report.tasks.iter().find(|t| t.id == id).unwrap();
    let unpublished = by_id("cargo-0");
    assert_eq!(
        unpublished.terminal,
        AnchorTerminal::Unsupported(RealHistoryUnsupported::SourceUnavailable),
        "a named pull request without its creation time leaves the repair's publication unestablished"
    );
    assert!(unpublished.audit.is_none());
    assert_eq!(
        by_id("cargo-1").terminal,
        AnchorTerminal::Skipped(RealHistorySkip::MissingCutoffEvidence),
        "a row whose pull request time is known is audited"
    );
    assert!(
        run.report.envelope.peaks.processes >= 1,
        "preparation's git children are counted in the process envelope even when nothing is graded"
    );
}

#[test]
fn the_time_study_measures_five_preparations_that_produced_a_snapshot() {
    static CLONES: AtomicUsize = AtomicUsize::new(0);
    fn counted_clone(entry: &AnchorEntry, into: &Path, deadline: Duration) -> std::io::Result<()> {
        CLONES.fetch_add(1, Ordering::SeqCst);
        clone_local(entry, into, deadline)
    }
    let dir = tempfile::tempdir().unwrap();
    // The first five rows fail their fetch fast; the study must look past
    // them before it projects the pilot's cost.
    let corpus = corpus(dir.path(), &[Variant::MissingIssue; 5]);
    let config = config(dir.path(), corpus, ControlScript::default(), 1);
    let host = Host {
        clone: counted_clone,
        ..PREPARING_HOST
    };
    match anchor::run(&config, host) {
        Err(RunError::StopForApproval(Affordability::StopForApproval { .. })) => {}
        Err(other) => panic!("expected a stop for approval, got {other:?}"),
        Ok(_) => panic!("expected a stop for approval, got a run"),
    }
    assert_eq!(
        CLONES.load(Ordering::SeqCst),
        10,
        "five unavailable rows are not a sample; the study stops after the fifth row that prepared"
    );
}

#[test]
fn the_tree_digest_keeps_paths_that_only_differ_in_bytes_apart() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join(std::ffi::OsStr::from_bytes(b"x\x80")), "same").unwrap();
    std::fs::write(b.join(std::ffi::OsStr::from_bytes(b"x\x81")), "same").unwrap();
    assert_ne!(
        anchor::tree_digest(&a).unwrap(),
        anchor::tree_digest(&b).unwrap(),
        "two names that differ only in bytes UTF-8 cannot carry are two entries"
    );
    let mut both = a.clone();
    both.push(std::ffi::OsStr::from_bytes(b"x\x81"));
    std::fs::write(&both, "same").unwrap();
    assert_ne!(
        anchor::tree_digest(&a).unwrap(),
        anchor::tree_digest(&b).unwrap(),
        "a tree with both names is not a tree with one"
    );
}

#[test]
fn missing_settings_and_an_unaccepted_witness_refuse_before_execution() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), &PLAIN);
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
    let mut climbing = config.clone();
    climbing.settings = settings(1);
    climbing.corpus.entries[0].id = "../escape".to_string();
    assert!(matches!(
        anchor::run(&climbing, HOST),
        Err(RunError::Corpus(AnchorError::NotAPathComponent { .. }))
    ));
    let mut no_namespaces = config.clone();
    no_namespaces.settings = settings(1);
    assert!(
        matches!(
            anchor::run(
                &no_namespaces,
                Host {
                    namespaces: || false,
                    ..HOST
                }
            ),
            Err(RunError::NoContainment)
        ),
        "a host without namespaces refuses before preparing anything"
    );
    assert!(!config.publish.exists());
}

#[test]
fn grading_runs_repository_code_without_the_runners_home_or_network() {
    if !suite_d::namespaces_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(workspace.join("src/lib.rs"), "").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let home = std::env::var("HOME").unwrap();
    let secret = dir.path().join("tasks/secret.txt");
    let probe = format!(
        r#"
#[test]
fn isolated() {{
    assert_ne!(std::env::var("HOME").ok().as_deref(), Some({home:?}));
    assert!(std::net::TcpStream::connect(("127.0.0.1", {port})).is_err());
    let own = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std::net::TcpStream::connect(own.local_addr().unwrap()).unwrap();
    assert!(std::fs::read({secret:?}).is_err(), "the task material is masked");
}}
"#
    );
    let mut limits = campaign::profile(Scale::S0, 128, 600_000, None).envelope;
    limits.processes = 2;
    let mut charges = campaign::Charges::new(limits);
    let tasks = dir.path().join("tasks");
    std::fs::create_dir_all(&tasks).unwrap();
    std::fs::write(tasks.join("secret.txt"), "the fix").unwrap();
    let layout = anchor::Layout {
        root: dir.path().to_path_buf(),
        private: dir.path().to_path_buf(),
        tasks: tasks.clone(),
        tree: workspace.clone(),
        target: dir.path().join("target"),
    };
    let results = anchor::grade(
        &workspace,
        &[("probe".to_string(), probe)],
        &[],
        &layout,
        "probe",
        &mut charges,
    )
    .unwrap();
    assert_eq!(
        results["probe"],
        HiddenOutcome::Passed,
        "the runner's home, its loopback listener, and the task material are out of reach; the test's own loopback works"
    );
    drop(listener);
}

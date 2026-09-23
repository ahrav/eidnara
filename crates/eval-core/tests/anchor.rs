//! Real-history anchors: the identifier-only corpus, the time study, the
//! cutoff audit, the insufficiency proof, the no-repository control, and the
//! per-provider accounting that feeds the claim class.

use std::collections::{BTreeMap, BTreeSet};

use eval_core::{
    ANCHOR_CORPUS_SCHEMA, Affordability, AnchorCorpus, AnchorEntry, AnchorError, AnchorRole,
    AnchorVerdict, ClaimClass, ClassifiedControl, Contamination, ControlRefused, ControlVerdict,
    CutoffAudit, CutoffRefused, DisabledReason, Family, HiddenOutcome, InsufficiencyProof,
    InsufficiencyRefused, NoRepositoryControl, PILOT_COMPOSITION, Preparation, ProviderProfile,
    RealHistorySettings, SettingsRefused, SkipReason, TIME_STUDY_TASKS, Terminal, TimeStudyRefused,
    TransferCriterion, UnmetClause, UnsupportedReason, WorldProvenance, anchor_set,
    classify_control, derive_claim_class, future_answers, time_study,
};
use serde_json::json;

const CUTOFF: i64 = 1_700_000_000_000;

type Mutate = fn(&mut CutoffAudit);

fn sha(byte: u8) -> String {
    format!("{byte:02x}").repeat(20)
}

fn entry(id: &str, family: Family, byte: u8) -> AnchorEntry {
    AnchorEntry {
        id: id.to_string(),
        family,
        repository: format!("https://example.invalid/{}/repo.git", family.label()),
        license: "MIT".to_string(),
        base_sha: sha(byte),
        fix_sha: sha(byte + 1),
        issue: 1_000 + u64::from(byte),
        pull_request: Some(2_000 + u64::from(byte)),
        cutoff_ms: CUTOFF,
    }
}

fn pilot() -> AnchorCorpus {
    let mut entries = Vec::new();
    let mut byte = 0x10;
    for (family, count) in PILOT_COMPOSITION {
        for index in 0..count {
            entries.push(entry(&format!("{}-{index}", family.label()), family, byte));
            byte += 2;
        }
    }
    AnchorCorpus {
        schema: ANCHOR_CORPUS_SCHEMA.to_string(),
        entries,
    }
}

fn provider() -> ProviderProfile {
    ProviderProfile {
        provider: "anthropic".to_string(),
        model: "claude-x".to_string(),
        tokenizer_profile: "tp-1".to_string(),
    }
}

fn audit(task: &str) -> CutoffAudit {
    CutoffAudit {
        task: task.to_string(),
        cutoff_ms: CUTOFF,
        base_committed_ms: CUTOFF - 86_400_000,
        fix_committed_ms: CUTOFF + 3_600_000,
        issue_created_ms: CUTOFF - 7_200_000,
        snapshot_digest: "ab".repeat(32),
        fix_paths_present: false,
    }
}

fn proof(task: &str) -> InsufficiencyProof {
    InsufficiencyProof {
        task: task.to_string(),
        hidden: BTreeMap::from([
            ("regression".to_string(), HiddenOutcome::Failed),
            ("smoke".to_string(), HiddenOutcome::Passed),
        ]),
    }
}

fn control(task: &str, terminal: Terminal) -> NoRepositoryControl {
    NoRepositoryControl {
        task: task.to_string(),
        provider: provider(),
        execution_image: "image-1".to_string(),
        analysis_family_digest: "cd".repeat(32),
        terminal,
        repository_access: Vec::new(),
        future_answers: Vec::new(),
    }
}

fn classified(task: &str, verdict: ControlVerdict) -> ClassifiedControl {
    ClassifiedControl {
        task: task.to_string(),
        provider: provider(),
        verdict,
    }
}

fn verdict_of(
    control: &NoRepositoryControl,
    comparison: &NoRepositoryControl,
) -> Result<ControlVerdict, ControlRefused> {
    classify_control(control, comparison).map(|c| c.verdict)
}

type Evidence = (
    BTreeMap<String, CutoffAudit>,
    BTreeMap<String, InsufficiencyProof>,
    BTreeMap<String, ClassifiedControl>,
);

fn evidence(corpus: &AnchorCorpus) -> Evidence {
    let ids = || corpus.entries.iter().map(|e| e.id.clone());
    (
        ids().map(|id| (id.clone(), audit(&id))).collect(),
        ids().map(|id| (id.clone(), proof(&id))).collect(),
        ids()
            .map(|id| (id.clone(), classified(&id, ControlVerdict::Eligible)))
            .collect(),
    )
}

#[test]
fn the_corpus_persists_identifiers_only_and_is_the_pilot_composition() {
    let corpus = pilot();
    corpus.validate().unwrap();
    corpus.is_pilot().unwrap();
    assert_eq!(corpus.entries.len(), 20);
    let value = serde_json::to_value(&corpus).unwrap();
    let text = value.to_string();
    for forbidden in ["statement", "issue_text", "body", "diff", "title"] {
        assert!(
            !text.contains(&format!("\"{forbidden}\"")),
            "{forbidden} is not a corpus field"
        );
    }
    assert_eq!(value["entries"][0]["base_sha"], json!(sha(0x10)));

    let mut text_row = corpus.clone();
    text_row.entries[0].repository =
        "The issue says the parser panics on empty input\nsee below".to_string();
    assert_eq!(
        text_row.validate(),
        Err(AnchorError::TextPersisted {
            id: "cargo-0".to_string(),
            field: "repository"
        })
    );
    let mut short_sha = corpus.clone();
    short_sha.entries[1].fix_sha = "abc".to_string();
    assert!(matches!(
        short_sha.validate(),
        Err(AnchorError::NotASha {
            field: "fix_sha",
            ..
        })
    ));
    let mut duplicate = corpus.clone();
    duplicate.entries[2].id = "cargo-0".to_string();
    assert!(matches!(
        duplicate.validate(),
        Err(AnchorError::DuplicateId { .. })
    ));
    let mut not_pilot = corpus.clone();
    not_pilot.entries.pop();
    assert!(matches!(
        not_pilot.is_pilot(),
        Err(AnchorError::NotPilotComposition { .. })
    ));
    assert_ne!(corpus.digest(), not_pilot.digest());
}

#[test]
fn the_time_study_projects_the_pilot_and_stops_for_approval_past_the_bound() {
    let corpus = pilot();
    let measured: Vec<Preparation> = corpus.entries[..TIME_STUDY_TASKS]
        .iter()
        .map(|e| Preparation {
            task: e.id.clone(),
            prepare_ms: 600_000,
        })
        .collect();
    assert_eq!(
        time_study(&corpus, &measured, 20 * 600_000).unwrap(),
        Affordability::Affordable {
            projected_ms: 20 * 600_000
        }
    );
    assert_eq!(
        time_study(&corpus, &measured, 20 * 600_000 - 1).unwrap(),
        Affordability::StopForApproval {
            projected_ms: 20 * 600_000,
            bound_ms: 20 * 600_000 - 1
        }
    );
    assert_eq!(
        time_study(&corpus, &measured[..4], u64::MAX),
        Err(TimeStudyRefused::WrongTaskCount { measured: 4 })
    );
    let mut stranger = measured.clone();
    stranger[0].task = "not-in-corpus".to_string();
    assert_eq!(
        time_study(&corpus, &stranger, u64::MAX),
        Err(TimeStudyRefused::NotFromCorpus {
            task: "not-in-corpus".to_string()
        })
    );
}

#[test]
fn the_cutoff_audit_excludes_future_code_and_future_issue_knowledge() {
    let good = audit("cargo-0");
    good.validate().unwrap();
    let cases: [(Mutate, CutoffRefused); 5] = [
        (
            |a| a.base_committed_ms = a.cutoff_ms + 1,
            CutoffRefused::BaseAfterCutoff,
        ),
        (
            |a| a.fix_committed_ms = a.cutoff_ms,
            CutoffRefused::FixNotAfterCutoff,
        ),
        (
            |a| a.issue_created_ms = a.cutoff_ms + 1,
            CutoffRefused::IssueAfterCutoff,
        ),
        (
            |a| a.fix_paths_present = true,
            CutoffRefused::FutureContentInSnapshot,
        ),
        (
            |a| a.snapshot_digest.clear(),
            CutoffRefused::SnapshotDigestMissing,
        ),
    ];
    for (mutate, expected) in cases {
        let mut audit = good.clone();
        mutate(&mut audit);
        assert_eq!(audit.validate(), Err(expected));
    }
    assert_eq!(
        serde_json::to_value(CutoffRefused::IssueAfterCutoff).unwrap(),
        json!({"reason": "issue_after_cutoff"})
    );
}

#[test]
fn the_insufficiency_proof_is_an_executed_failing_run() {
    proof("cargo-0").validate().unwrap();
    let mut passing = proof("cargo-0");
    passing
        .hidden
        .insert("regression".to_string(), HiddenOutcome::Passed);
    assert_eq!(
        passing.validate(),
        Err(InsufficiencyRefused::TreeAlreadyPasses)
    );
    let empty = InsufficiencyProof {
        task: "cargo-0".to_string(),
        hidden: BTreeMap::new(),
    };
    assert_eq!(
        empty.validate(),
        Err(InsufficiencyRefused::NothingExecuted),
        "a corpus row without a run is not a proof"
    );
    let errored = InsufficiencyProof {
        task: "cargo-0".to_string(),
        hidden: BTreeMap::from([("regression".to_string(), HiddenOutcome::Errored)]),
    };
    assert_eq!(
        errored.validate(),
        Err(InsufficiencyRefused::NothingExecuted),
        "an errored test never reached a verdict"
    );
    let mut no_failure = proof("cargo-0");
    no_failure
        .hidden
        .insert("regression".to_string(), HiddenOutcome::Errored);
    assert_eq!(
        no_failure.validate(),
        Err(InsufficiencyRefused::TreeAlreadyPasses),
        "a passing test beside an errored one shows no failure"
    );
    let mut failed_and_errored = proof("cargo-0");
    failed_and_errored
        .hidden
        .insert("build".to_string(), HiddenOutcome::Errored);
    failed_and_errored.validate().unwrap();
}

#[test]
fn a_control_marks_memorized_tasks_and_detects_seeded_contamination() {
    let comparison = control("cargo-0", Terminal::Fail);
    assert_eq!(
        classify_control(&control("cargo-0", Terminal::Fail), &comparison),
        Ok(classified("cargo-0", ControlVerdict::Eligible)),
        "the verdict carries the task and provider it was judged for"
    );
    assert_eq!(
        verdict_of(&control("cargo-0", Terminal::Pass), &comparison),
        Ok(ControlVerdict::Excluded {
            contamination: Contamination::Memorized
        }),
        "the statement alone sufficed"
    );
    let mut reached = control("cargo-0", Terminal::Fail);
    reached.repository_access = vec!["src/lib.rs".to_string()];
    assert!(matches!(
        verdict_of(&reached, &comparison),
        Ok(ControlVerdict::Excluded {
            contamination: Contamination::RepositoryAccess { .. }
        })
    ));
    let entry = entry("cargo-0", Family::Cargo, 0x10);
    let answers = future_answers(
        &entry,
        &format!("fixed upstream in {} via #{}", &entry.fix_sha[..12], 2_016),
    );
    assert_eq!(answers.len(), 2);
    assert!(future_answers(&entry, "no idea").is_empty());
    let mut future = control("cargo-0", Terminal::Fail);
    future.future_answers = answers;
    assert!(matches!(
        verdict_of(&future, &comparison),
        Ok(ControlVerdict::Excluded {
            contamination: Contamination::FutureAnswer { .. }
        })
    ));
    let mut other_image = control("cargo-0", Terminal::Fail);
    other_image.execution_image = "image-2".to_string();
    assert_eq!(
        verdict_of(&other_image, &comparison),
        Err(ControlRefused::NotComparable {
            field: "execution_image"
        })
    );
    let mut other_provider = control("cargo-0", Terminal::Fail);
    other_provider.provider.model = "other".to_string();
    assert_eq!(
        verdict_of(&other_provider, &comparison),
        Err(ControlRefused::NotComparable { field: "provider" })
    );
    let censored = control(
        "cargo-0",
        Terminal::Censored {
            reason: eval_core::CensorReason::HardDeadlineMs,
        },
    );
    assert_eq!(
        verdict_of(&censored, &comparison),
        Ok(ControlVerdict::Eligible),
        "a censored control is not memorized"
    );
    for terminal in [
        Terminal::Indeterminate,
        Terminal::Skipped(SkipReason::MissingCutoffEvidence),
        Terminal::Unsupported(UnsupportedReason::SourceUnavailable),
        Terminal::Disabled(DisabledReason::FeatureOff),
    ] {
        assert_eq!(
            verdict_of(&control("cargo-0", terminal), &comparison),
            Err(ControlRefused::NotRun { terminal }),
            "a control that never ran is not eligible: {terminal:?}"
        );
    }
}

#[test]
fn the_pilot_alone_never_transfers_and_exclusions_keep_their_accounting() {
    let corpus = pilot();
    let (audits, proofs, mut controls) = evidence(&corpus);
    controls.insert(
        "tokio-3".to_string(),
        classified(
            "tokio-3",
            ControlVerdict::Excluded {
                contamination: Contamination::Memorized,
            },
        ),
    );
    let mut audits_with_failure = audits.clone();
    audits_with_failure
        .get_mut("django-1")
        .unwrap()
        .fix_paths_present = true;
    let mut proofs_missing = proofs.clone();
    proofs_missing.remove("cargo-7");

    let (set, accounting) = anchor_set(
        &corpus,
        AnchorRole::Pilot,
        &audits_with_failure,
        &proofs_missing,
        &controls,
        &provider(),
    );
    assert_eq!(set.tasks.len(), 20, "every task keeps its row");
    let verdict = |id: &str| set.tasks.iter().find(|t| t.id == id).unwrap().verdict;
    assert_eq!(verdict("tokio-3"), AnchorVerdict::Residue);
    assert_eq!(verdict("django-1"), AnchorVerdict::CutoffInvalid);
    assert_eq!(verdict("cargo-7"), AnchorVerdict::Residue);
    assert_eq!(verdict("cargo-0"), AnchorVerdict::Valid);
    assert_eq!(accounting.eligible.len(), 17);
    assert_eq!(accounting.excluded["tokio-3"], Contamination::Memorized);
    assert_eq!(
        accounting.cutoff_invalid["django-1"],
        CutoffRefused::FutureContentInSnapshot
    );
    assert_eq!(
        accounting.insufficiency_missing,
        BTreeSet::from(["cargo-7".to_string()])
    );
    assert!(accounting.insufficiency_refused.is_empty());
    assert!(accounting.control_missing.is_empty());

    let criterion = TransferCriterion {
        approved_by: "maintainer".to_string(),
        approved_at_run_id: "ab".repeat(32),
        min_valid_tasks: 17,
        required_families: ["cargo", "tokio", "django"]
            .map(String::from)
            .into_iter()
            .collect(),
    };
    let pilot_claim =
        derive_claim_class(WorldProvenance::RealHistory, Some(&set), Some(&criterion));
    assert_eq!(pilot_claim.class, ClaimClass::GeneratedPhase1);
    assert!(
        pilot_claim.unmet.contains(&UnmetClause::AnchorSetIsPilot),
        "the pilot alone never transfers"
    );
    assert!(pilot_claim.unmet.contains(&UnmetClause::AnchorTaskNotValid));
    assert_eq!(pilot_claim.skipped.len(), 3);

    let (clean, _) = anchor_set(
        &corpus,
        AnchorRole::Transfer,
        &audits,
        &proofs,
        &controls,
        &provider(),
    );
    let excluded_claim =
        derive_claim_class(WorldProvenance::RealHistory, Some(&clean), Some(&criterion));
    assert_eq!(
        excluded_claim.class,
        ClaimClass::GeneratedPhase1,
        "the memorized task is not valid for this pair"
    );
    assert_eq!(excluded_claim.skipped, vec!["tokio-3".to_string()]);
    controls.insert(
        "tokio-3".to_string(),
        classified("tokio-3", ControlVerdict::Eligible),
    );
    let (full, _) = anchor_set(
        &corpus,
        AnchorRole::Transfer,
        &audits,
        &proofs,
        &controls,
        &provider(),
    );
    assert_eq!(
        derive_claim_class(WorldProvenance::RealHistory, Some(&full), Some(&criterion)).class,
        ClaimClass::Transfer
    );
    assert_eq!(
        derive_claim_class(WorldProvenance::RealHistory, Some(&full), None).unmet,
        vec![UnmetClause::NoTransferCriterion],
        "a frozen approved criterion is required"
    );
}

#[test]
fn evidence_counts_only_for_the_corpus_task_and_cutoff_it_names() {
    let corpus = pilot();
    let (mut audits, mut proofs, controls) = evidence(&corpus);
    audits.insert("cargo-0".to_string(), audit("tokio-0"));
    let later = audits.get_mut("cargo-1").unwrap();
    later.cutoff_ms = CUTOFF + 36_000_000;
    later.base_committed_ms = CUTOFF + 3_600_000;
    later.fix_committed_ms = CUTOFF + 72_000_000;
    later
        .validate()
        .expect("the audit passes against its own later cutoff");
    proofs.insert("cargo-2".to_string(), proof("tokio-2"));

    let (set, accounting) = anchor_set(
        &corpus,
        AnchorRole::Transfer,
        &audits,
        &proofs,
        &controls,
        &provider(),
    );
    let verdict = |id: &str| set.tasks.iter().find(|t| t.id == id).unwrap().verdict;
    assert_eq!(verdict("cargo-0"), AnchorVerdict::CutoffInvalid);
    assert_eq!(
        verdict("cargo-1"),
        AnchorVerdict::CutoffInvalid,
        "a base committed after the row's cutoff is future code"
    );
    assert_eq!(verdict("cargo-2"), AnchorVerdict::Residue);
    assert_eq!(
        accounting.cutoff_invalid,
        BTreeMap::from([
            ("cargo-0".to_string(), CutoffRefused::AuditForOtherTask),
            ("cargo-1".to_string(), CutoffRefused::CutoffMismatch),
        ])
    );
    assert_eq!(
        accounting.insufficiency_refused,
        BTreeMap::from([(
            "cargo-2".to_string(),
            InsufficiencyRefused::ProofForOtherTask
        )])
    );
    assert_eq!(accounting.eligible.len(), 17);
}

#[test]
fn a_control_qualifies_only_the_task_and_provider_it_was_run_for() {
    let corpus = pilot();
    let (audits, proofs, _) = evidence(&corpus);
    let controls: BTreeMap<String, ClassifiedControl> = corpus
        .entries
        .iter()
        .map(|e| {
            let run = control(&e.id, Terminal::Fail);
            (e.id.clone(), classify_control(&run, &run).unwrap())
        })
        .collect();
    let mut other = provider();
    other.model = "claude-y".to_string();

    let (set, accounting) = anchor_set(
        &corpus,
        AnchorRole::Transfer,
        &audits,
        &proofs,
        &controls,
        &other,
    );
    assert!(
        set.tasks
            .iter()
            .all(|t| t.verdict == AnchorVerdict::Residue),
        "another provider's controls say nothing about this one"
    );
    assert!(accounting.eligible.is_empty());
    assert_eq!(accounting.control_missing.len(), 20);

    let (set, _) = anchor_set(
        &corpus,
        AnchorRole::Transfer,
        &audits,
        &proofs,
        &controls,
        &provider(),
    );
    assert!(set.tasks.iter().all(|t| t.verdict == AnchorVerdict::Valid));

    let mut misfiled = controls;
    misfiled.insert(
        "cargo-0".to_string(),
        classified("cargo-1", ControlVerdict::Eligible),
    );
    let (_, accounting) = anchor_set(
        &corpus,
        AnchorRole::Transfer,
        &audits,
        &proofs,
        &misfiled,
        &provider(),
    );
    assert_eq!(
        accounting.control_missing,
        BTreeSet::from(["cargo-0".to_string()])
    );
}

#[test]
fn accounting_names_a_missing_control_and_a_refused_proof_apart() {
    let corpus = pilot();
    let (audits, mut proofs, mut controls) = evidence(&corpus);
    proofs
        .get_mut("cargo-1")
        .unwrap()
        .hidden
        .insert("regression".to_string(), HiddenOutcome::Passed);
    proofs.remove("cargo-2");
    controls.remove("cargo-0");

    let (set, accounting) = anchor_set(
        &corpus,
        AnchorRole::Transfer,
        &audits,
        &proofs,
        &controls,
        &provider(),
    );
    assert_eq!(
        accounting.control_missing,
        BTreeSet::from(["cargo-0".to_string()])
    );
    assert_eq!(
        accounting.insufficiency_refused,
        BTreeMap::from([(
            "cargo-1".to_string(),
            InsufficiencyRefused::TreeAlreadyPasses
        )])
    );
    assert_eq!(
        accounting.insufficiency_missing,
        BTreeSet::from(["cargo-2".to_string()])
    );
    assert_eq!(accounting.eligible.len(), 17);
    assert_eq!(
        set.tasks
            .iter()
            .filter(|t| t.verdict == AnchorVerdict::Residue)
            .count(),
        3
    );
    let value = serde_json::to_value(&accounting).unwrap();
    assert_eq!(
        value["insufficiency_refused"]["cargo-1"],
        json!({"reason": "tree_already_passes"})
    );
    assert_eq!(value["control_missing"], json!(["cargo-0"]));
}

#[test]
fn future_pull_requests_match_whole_numbers_and_repository_urls() {
    let mut entry = entry("cargo-0", Family::Cargo, 0x10);
    entry.pull_request = Some(20);
    assert!(
        future_answers(&entry, "see #2016 and #201").is_empty(),
        "#20 is not inside #2016"
    );
    assert_eq!(future_answers(&entry, "see #20."), vec!["pull_request:20"]);
    assert_eq!(
        future_answers(&entry, "see #2016, then #20"),
        vec!["pull_request:20"]
    );
    entry.pull_request = Some(2016);
    for output in [
        "fixed in https://example.invalid/cargo/repo/pull/2016",
        "see example.invalid/cargo/repo/pull/2016/files",
    ] {
        assert_eq!(
            future_answers(&entry, output),
            vec!["pull_request:2016"],
            "{output}"
        );
    }
    for output in [
        "https://example.invalid/cargo/repo/pull/20160",
        "https://example.invalid/other/repo/pull/2016",
    ] {
        assert!(future_answers(&entry, output).is_empty(), "{output}");
    }
}

#[test]
fn future_answers_skips_a_fix_sha_that_was_never_validated() {
    let mut entry = entry("cargo-0", Family::Cargo, 0x10);
    entry.fix_sha = "abc".to_string();
    assert!(future_answers(&entry, "abc").is_empty());
    entry.fix_sha = format!("a{}", "é".repeat(6));
    assert!(
        future_answers(&entry, "a").is_empty(),
        "byte 12 splits a character"
    );
}

#[test]
fn settings_refuse_before_execution_and_reasons_are_typed() {
    let settings = RealHistorySettings {
        providers: vec![provider()],
        execution_image: "image-1".to_string(),
        preparation_bound_ms: Some(1),
        transfer_criterion: None,
    };
    settings.validate().unwrap();
    let mut no_providers = settings.clone();
    no_providers.providers.clear();
    assert_eq!(no_providers.validate(), Err(SettingsRefused::NoProviders));
    let mut no_image = settings.clone();
    no_image.execution_image = " ".to_string();
    assert_eq!(no_image.validate(), Err(SettingsRefused::NoExecutionImage));
    let mut no_bound = settings;
    no_bound.preparation_bound_ms = None;
    assert_eq!(
        no_bound.validate(),
        Err(SettingsRefused::NoPreparationBound)
    );
    assert_eq!(
        serde_json::to_value(Terminal::Skipped(SkipReason::MissingCutoffEvidence)).unwrap(),
        json!({"kind": "skipped", "reason": "missing_cutoff_evidence"})
    );
    assert_eq!(
        serde_json::to_value(Terminal::Unsupported(UnsupportedReason::SourceUnavailable)).unwrap(),
        json!({"kind": "unsupported", "reason": "source_unavailable"})
    );
    assert_eq!(
        serde_json::to_value(Terminal::Unsupported(
            UnsupportedReason::UnsupportedRuntime {
                family: Family::Django
            }
        ))
        .unwrap(),
        json!({"kind": "unsupported", "reason": "unsupported_runtime", "family": "django"})
    );
    assert_eq!(provider().key(), "anthropic/claude-x@tp-1");
}

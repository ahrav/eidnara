//! One Suite B campaign at scale S0 on the default surface, driven through
//! the shell the `eval_runner` example ships: what the run found is asserted
//! here, in-process, and once more from the example's own command line.

#![cfg(all(unix, feature = "test-support"))]

mod support;

#[path = "../examples/eval_runner/campaign.rs"]
mod campaign;

use std::collections::BTreeMap;

use campaign::{Config, MANIFEST_FILE, PLANTED_SLOT, REPORT_FILE, Run, RunError, profile};
use eval_core::{
    Analysis, Approval, ArmKind, ArmResult, AxisValue, Carrier, Cut, DisabledReason, Established,
    HistoryPolicy, IntervalOutcome, ProfileError, Ratio, ReportOutcome, SampleLedger, SampleRecord,
    Scale, SkipReason, StageVerdict, Surface1Stage, Terminal, parse_manifest, parse_report,
};
use support::direct_host::example_binary;
use support::publish::staged_path;

const AGED_MESSAGES: u32 = 130;
const S0_ELAPSED_BOUND_MS: u64 = 1_200_000;
const S1_AGED_MESSAGES: u32 = 400;

/// The budget the scale's environment variable grants, which becomes the
/// profile's elapsed bound so the envelope refuses the first reading past it.
/// Without one the campaign is disabled at its scale, which is the
/// closed-vocabulary terminal the sample ledger records, and `None` says so;
/// a budget that is set but not a number is refused rather than read as
/// absent.
fn budget(scale: Scale) -> Option<u64> {
    let variable = scale.budget_env();
    match std::env::var(variable) {
        Err(_) => {
            let record = SampleRecord {
                id: format!(
                    "{}-surface1-raw",
                    serde_json::to_value(scale).unwrap().as_str().unwrap()
                ),
                task: "campaign".to_string(),
                arm: ArmKind::Aged,
                policy: HistoryPolicy::Raw,
                cut: Cut::EndOfRun,
                lineage: vec![],
                terminal: Terminal::Disabled(DisabledReason::ScaleNotBudgeted { scale }),
            };
            let ledger = SampleLedger {
                epoch: 1,
                order: vec![record.id.clone()],
                samples: BTreeMap::from([(record.id.clone(), record)]),
            };
            assert_eq!(ledger.rates().unwrap().disabled, Ratio::ONE);
            None
        }
        Ok(text) => Some(
            text.parse::<u64>()
                .unwrap_or_else(|e| panic!("{variable}={text:?} is not a millisecond budget: {e}")),
        ),
    }
}

fn approval() -> Approval {
    Approval {
        approved_by: "test-approval".to_string(),
        approved_at_run_id: "ab".repeat(32),
    }
}

/// Runs one campaign in-process into a root of its own and checks what every
/// run must satisfy whatever its scale: the published report and manifest
/// read back equal, the manifest says how the arms reached the store and
/// refuses to be relabelled as seeded, every sample is accounted for, and the
/// raw arms on surface 1 are inert.
fn campaign(scale: Scale, aged_messages: u32, elapsed_bound_ms: u64) -> Run {
    let publish = tempfile::tempdir().unwrap();
    let run = campaign::run(&Config {
        scale,
        aged_messages,
        elapsed_bound_ms,
        approval: Some(approval()),
        publish: publish.path().to_path_buf(),
    })
    .unwrap();
    let report_path = publish.path().join(REPORT_FILE);
    assert!(!staged_path(&report_path).exists());
    let report_bytes = std::fs::read(&report_path).unwrap();
    assert_eq!(
        report_bytes, run.report_bytes,
        "the file is the bytes the run reports"
    );
    let published: serde_json::Value = serde_json::from_slice(&report_bytes).unwrap();
    assert_eq!(parse_report(&published).unwrap(), run.report);
    let peaks = &published["envelope"]["peaks"];
    for resource in ["store_bytes", "elapsed_ms", "artifact_bytes"] {
        assert!(
            peaks[resource].as_u64().unwrap() > 0,
            "{resource} on disk: {peaks}"
        );
    }
    assert_eq!(peaks["processes"], 1);
    assert_eq!(peaks["temp_roots"], 3);
    assert!(peaks["cassette_bytes"].as_u64().unwrap() > 0);
    assert_eq!(
        peaks["artifact_bytes"].as_u64().unwrap(),
        report_bytes.len() as u64,
        "the published peak is the published file's size"
    );

    let manifest_bytes = std::fs::read(publish.path().join(MANIFEST_FILE)).unwrap();
    assert_eq!(manifest_bytes, run.manifest_bytes);
    let read_back: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
    let parsed = parse_manifest(&read_back).unwrap();
    assert_eq!(parsed, run.manifest);
    assert_eq!(
        parsed.eval_run_id, run.report.eval_run_id,
        "the report and the manifest beside it carry one run identity"
    );
    assert_eq!(parsed.digest().unwrap(), run.manifest.digest().unwrap());
    assert_eq!(read_back["construction"], "replay");
    assert_eq!(read_back["ingestion"], "transform-route, turn by turn");
    // A seeded history may not call itself aged: the same manifest relabelled
    // as written straight into the store is refused.
    let mut relabelled = read_back.clone();
    relabelled["ingestion"] = serde_json::json!("direct-database, non-aged");
    assert_eq!(
        parse_manifest(&relabelled),
        Err(eval_core::ManifestError::DirectDatabaseAged)
    );

    // The five injection cases are planned for the task set and scored as a
    // surface-1 run observes them. The summary carrier's canary was planted
    // into a message the summarizer folds, so when the aged life recorded it
    // is ingested (the daemon's own segment carries it) and not retrieved (no
    // task asks in its words); the tool-output carrier's canary was planted
    // into a tool span's output and never reaches a segment; the other
    // carriers have no payload in this world and read not reached; nothing is
    // packed, quoted, or obeyed on surface 1, which has no packing, no model
    // output, and no mediation boundary.
    assert_eq!(run.report.injection.len(), 5);
    // A refused recording leaves whatever the life folded before the refusal
    // and no structured arm to retrieve from.
    let ingested = run.aged.covered.values().flatten().count() > PLANTED_SLOT as usize;
    let expected_summary = match (run.aged.refused, ingested) {
        (true, true) => (AxisValue::Yes, AxisValue::NotReached),
        (true, false) => (AxisValue::No, AxisValue::NotReached),
        (false, _) => (AxisValue::Yes, AxisValue::No),
    };
    for score in &run.report.injection {
        let carrier = Carrier::ALL
            .into_iter()
            .find(|carrier| score.case_id.contains(carrier.label()))
            .unwrap_or_else(|| panic!("{score:?}"));
        // The daemon presents a message's text to its summarizer and only the
        // names of its tool calls, never a tool result's output, so a canary
        // in a tool output is never folded into a segment.
        let expected = match carrier {
            Carrier::Summary => expected_summary,
            Carrier::ToolOutput => (AxisValue::No, AxisValue::NotReached),
            _ => (AxisValue::NotReached, AxisValue::NotReached),
        };
        assert_eq!((score.ingested, score.retrieved), expected, "{score:?}");
        assert_eq!(
            (score.packed, score.exposure, score.obeyed),
            (
                AxisValue::NotReached,
                AxisValue::NotReached,
                AxisValue::NotMeasurable
            ),
            "{score:?}"
        );
    }
    // The baseline contrast the compiler established: one falsification pair
    // and one positive control.
    let ReportOutcome::Open { gated } = &run.report.outcome else {
        panic!("{:?}", run.report.outcome);
    };
    assert_eq!(
        (
            gated.baseline.falsification_pairs_failed,
            gated.baseline.positive_controls_passed
        ),
        (1, 1)
    );
    // Eighteen samples: three pairs, two arms, three policies; the pruned
    // arms are declared and never attempted, and a refused structured
    // recording skips its arm's samples.
    let skipped = run
        .report
        .samples
        .samples
        .values()
        .filter(|s| matches!(s.terminal, Terminal::Skipped(SkipReason::RedactionRefused)))
        .count();
    assert_eq!(run.report.rates.samples, 18);
    assert_eq!(run.report.rates.unsupported, Ratio::new(1, 3));
    assert_eq!(skipped, if run.aged.refused { 3 } else { 0 });
    assert_eq!(run.report.samples.attempted(), 12 - skipped);

    // Surface 1 serves history segments and nothing else, and only the
    // summarizer writes them: under raw history no arm has a unit for any
    // truth, so every task is lost at the candidate window on both arms (with
    // no segment at all the window cannot hold the truth; this pins that the
    // three hint gates passed and the store held no unit), the pairs are
    // concordant, the paired gates see no loss, and the floor, which asks the
    // control to deliver at all, fails. The short control never reaches the
    // pressure the summarizer fires at, so its structured arm is as empty.
    let by_task: BTreeMap<&str, _> = run
        .outcomes
        .iter()
        .map(|o| (o.pair_id.as_str(), o))
        .collect();
    for pair in &run.set.pairs {
        let name = pair.task.id.as_str();
        assert_eq!(
            (by_task[name].aged, by_task[name].fresh),
            (ArmResult::Fail, ArmResult::Fail),
            "{name} raw"
        );
        for label in ["aged", "fresh", "fresh/structured"] {
            assert_eq!(
                run.verdicts[&(name.to_string(), label)],
                StageVerdict::FirstLoss(Surface1Stage::CandidateWindow),
                "{name} {label}"
            );
        }
    }
    let analysis = &gated.analysis;
    assert_eq!(
        (analysis.counts.n, analysis.counts.b, analysis.counts.c),
        (3, 0, 0)
    );
    assert!(
        analysis.gates.quality_loss.passed && analysis.gates.harm.passed,
        "{:?}",
        analysis.gates
    );
    assert!(
        !analysis.gates.floor.passed,
        "an inert control fails the floor: {:?}",
        analysis.gates
    );
    assert!(matches!(
        analysis.interval,
        IntervalOutcome::Withheld { .. }
    ));
    let Analysis::Report(_) = eval_core::analyze(
        &eval_core::FrozenFamily::freeze(&run.report.family).unwrap(),
        &run.report.family,
        &run.outcomes,
        &run.report.arm_rates,
    )
    .unwrap() else {
        panic!("the report's analysis is the family's over the raw outcomes");
    };

    // On the structured aged arm the daemon's summarizer folded the older
    // history `CHUNK` messages to a segment: a truth at the head of its
    // segment is served whole, a truth folded past the fragment cap reaches
    // render with its words cut off, and a truth in the protected tail has no
    // unit at all. A refused recording leaves no structured pairs to judge.
    assert_eq!(
        run.structured_outcomes.len(),
        if run.aged.refused {
            0
        } else {
            run.set.pairs.len()
        },
        "structured pairs are judged only when both arms ran"
    );
    let structured_by_task: BTreeMap<&str, _> = run
        .structured_outcomes
        .iter()
        .map(|o| (o.pair_id.as_str(), o))
        .collect();
    let folded = |name: &str| {
        let evidence = run
            .set
            .pairs
            .iter()
            .find(|p| p.task.id == name)
            .unwrap()
            .task
            .evidence
            .iter()
            .next()
            .unwrap();
        run.aged
            .covered
            .values()
            .find(|ids| ids.contains(evidence))
            .map(|ids| ids.iter().position(|id| id == evidence).unwrap())
    };
    for pair in run.set.pairs.iter().filter(|_| !run.aged.refused) {
        let name = pair.task.id.as_str();
        assert_eq!(structured_by_task[name].fresh, ArmResult::Fail, "{name}");
        let expected = match folded(name) {
            None => (
                ArmResult::Fail,
                StageVerdict::FirstLoss(Surface1Stage::CandidateWindow),
            ),
            Some(0) => (ArmResult::Pass, StageVerdict::Clean),
            Some(_) => (
                ArmResult::Fail,
                StageVerdict::FirstLoss(Surface1Stage::Render),
            ),
        };
        assert_eq!(
            (
                structured_by_task[name].aged,
                run.verdicts[&(name.to_string(), "aged/structured")]
            ),
            expected,
            "{name} aged/structured"
        );
    }
    // The falsifier is the third message, folded third into the first
    // segment; the positive control is the last message, in the protected
    // tail.
    if !run.aged.refused {
        assert_eq!(folded("early-message"), Some(2));
        assert_eq!(folded("last-message"), None);
    }
    run
}

#[test]
fn an_unapproved_profile_runs_no_campaign() {
    let unapproved = profile(Scale::S0, 512, 1_200_000, None);
    assert_eq!(
        unapproved.approved(),
        Err(ProfileError::NotApproved {
            name: "s0-surface1-raw".to_string(),
        })
    );
    let publish = tempfile::tempdir().unwrap();
    let config = Config {
        scale: Scale::S0,
        aged_messages: AGED_MESSAGES,
        elapsed_bound_ms: S0_ELAPSED_BOUND_MS,
        approval: None,
        publish: publish.path().to_path_buf(),
    };
    assert_eq!(
        campaign::run(&config).err(),
        Some(RunError::Profile(ProfileError::NotApproved {
            name: "s0-surface1-raw".to_string(),
        }))
    );
    // An approval with no approver is not one either, and a history the
    // window would swallow whole cannot hold a falsifier.
    let no_approver = Config {
        approval: Some(Approval {
            approved_by: String::new(),
            approved_at_run_id: "ab".repeat(32),
        }),
        ..config.clone()
    };
    assert_eq!(
        campaign::run(&no_approver).err(),
        Some(RunError::Profile(ProfileError::Empty {
            field: "approval.approved_by",
        }))
    );
    let short = Config {
        aged_messages: 100,
        approval: Some(approval()),
        ..config
    };
    assert_eq!(
        campaign::run(&short).err(),
        Some(RunError::AgedHistoryTooShort {
            aged_messages: 100,
            window: 100,
        })
    );
    assert_eq!(std::fs::read_dir(publish.path()).unwrap().count(), 0);
}

#[test]
fn a_publish_target_the_shell_cannot_write_is_refused_before_anything_runs() {
    let root = tempfile::tempdir().unwrap();
    let config = Config {
        scale: Scale::S0,
        aged_messages: AGED_MESSAGES,
        elapsed_bound_ms: S0_ELAPSED_BOUND_MS,
        approval: Some(approval()),
        publish: root.path().join("report-file"),
    };
    std::fs::write(&config.publish, b"not a directory").unwrap();
    let started = std::time::Instant::now();
    match campaign::run(&config).err() {
        Some(RunError::Publish { path, .. }) => assert_eq!(path, config.publish),
        other => panic!("a file is not a publish directory: {other:?}"),
    }

    let publish = root.path().join("published");
    std::fs::create_dir(&publish).unwrap();
    let staged = staged_path(&publish.join(REPORT_FILE));
    std::fs::write(&staged, b"{").unwrap();
    let leftover = Config {
        publish: publish.clone(),
        ..config.clone()
    };
    assert_eq!(
        campaign::run(&leftover).err(),
        Some(RunError::Publish {
            path: staged.clone(),
            kind: std::io::ErrorKind::AlreadyExists,
        })
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "both refusals came before a fixture was started"
    );
    assert_eq!(std::fs::read(&staged).unwrap(), b"{");
    assert_eq!(std::fs::read_dir(&publish).unwrap().count(), 1);

    // A prior run's report or manifest is refused too: a rename over it
    // would pair one generation's report with another's manifest.
    for file in [REPORT_FILE, MANIFEST_FILE] {
        let publish = root.path().join(format!("published-{file}"));
        std::fs::create_dir(&publish).unwrap();
        let prior = publish.join(file);
        std::fs::write(&prior, b"{}").unwrap();
        let started = std::time::Instant::now();
        assert_eq!(
            campaign::run(&Config {
                publish: publish.clone(),
                ..config.clone()
            })
            .err(),
            Some(RunError::Publish {
                path: prior.clone(),
                kind: std::io::ErrorKind::AlreadyExists,
            })
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert_eq!(std::fs::read(&prior).unwrap(), b"{}");
        assert_eq!(std::fs::read_dir(&publish).unwrap().count(), 1);
    }
}

/// An S0 campaign driven through the daemon's lifecycle takes longer than the
/// rest of the daemon's suite, so under the parent's nextest regression policy
/// it runs only where `EIDNARA_EVAL_S0_BUDGET_MS` grants it a budget: its own
/// CI job, or a developer who asks for it.
#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn an_s0_campaign_on_the_default_surface_publishes_one_gated_report() {
    let Some(budget_ms) = budget(Scale::S0) else {
        return;
    };
    // Two campaigns of one identity run at once from one checkout, each on
    // its own roots, cassette directory, and publish directory; they must
    // agree on the run identity, the result digest, the manifest digest, and
    // everything the report says, and differ only in their measurements.
    let (run, twin) = std::thread::scope(|scope| {
        let twin = scope.spawn(|| campaign(Scale::S0, AGED_MESSAGES, budget_ms));
        let run = campaign(Scale::S0, AGED_MESSAGES, budget_ms);
        (run, twin.join().unwrap())
    });
    assert_eq!(run.report.eval_run_id, twin.report.eval_run_id);
    assert_eq!(run.manifest.result_digest, twin.manifest.result_digest);
    assert_eq!(
        run.manifest.digest().unwrap(),
        twin.manifest.digest().unwrap(),
        "the manifest digest carries no clock"
    );
    assert_eq!(run.report.samples, twin.report.samples);
    assert_eq!(run.report.outcome, twin.report.outcome);
    assert_eq!(run.report.claims, twin.report.claims);
    assert_eq!(run.report.injection, twin.report.injection);
    assert_eq!(run.report.envelope.bounds, twin.report.envelope.bounds);
    assert_eq!(run.verdicts, twin.verdicts);
    let report = &run.report;
    assert!(report.envelope.peaks.elapsed_ms <= budget_ms);
    assert_eq!(report.profile.name, "s0-surface1-raw");
    assert_eq!(
        report.claims.established,
        vec![
            Established::FirstLossStageNamed,
            Established::TaskOraclePasses
        ]
    );
    assert_eq!(
        report.reachability(),
        eval_core::Reachability::DefaultProduction
    );
    // The trigger fires eight times over the aged life, projected headroom
    // then the force band, folding the older history into twenty-two
    // segments (a tool span on every tenth message adds to the tokens the
    // trigger weighs);
    // no frame is refused at this scale, and the plain task's message heads
    // its segment.
    assert!(!run.aged.refused);
    assert_eq!((run.aged.firings, run.aged.covered.len()), (8, 22));
    let plain = run
        .set
        .pairs
        .iter()
        .find(|p| p.task.id == "recent-message")
        .unwrap()
        .task
        .evidence
        .iter()
        .next()
        .unwrap();
    assert_eq!(
        run.aged
            .covered
            .values()
            .find(|ids| ids.contains(plain))
            .map(|ids| ids.iter().position(|id| id == plain)),
        Some(Some(0)),
        "the plain task's message heads its segment"
    );
}

/// The example's own command line runs the same shell: it publishes the S0
/// report and manifest into the directory it is given and answers with one
/// JSON line naming them, and the published manifest parses.
#[test]
#[ignore = "S0 runs under an explicit budget: set EIDNARA_EVAL_S0_BUDGET_MS and run with --ignored"]
fn the_eval_runner_example_publishes_the_s0_campaign() {
    let Some(budget_ms) = budget(Scale::S0) else {
        return;
    };
    let binary = example_binary("eval_runner", "eval-runner");
    let publish = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(&binary)
        .args([
            "campaign",
            "--scale",
            "s0",
            "--aged-messages",
            &AGED_MESSAGES.to_string(),
            "--elapsed-bound-ms",
            &budget_ms.to_string(),
            "--approved-by",
            "test-approval",
            "--approval-run-id",
            &"ab".repeat(32),
            "--publish",
            publish.path().to_str().unwrap(),
        ])
        .output()
        .expect("the example runs");
    assert!(
        output.status.success(),
        "eval_runner campaign failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["status"], "completed");
    assert_eq!(
        (
            &summary["pairs"],
            &summary["samples"],
            &summary["attempted"]
        ),
        (&3.into(), &18.into(), &12.into())
    );
    assert_eq!(summary["aged_summarizer"]["refused"], false);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(publish.path().join(MANIFEST_FILE)).unwrap())
            .unwrap();
    let parsed = parse_manifest(&manifest).unwrap();
    assert_eq!(summary["eval_run_id"], parsed.eval_run_id);
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(publish.path().join(REPORT_FILE)).unwrap()).unwrap();
    parse_report(&report).unwrap();
    // The example refuses a campaign nobody wrote down.
    let refused = std::process::Command::new(&binary)
        .args(["campaign", "--scale", "s0"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("--aged-messages is required"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(refused.status.code(), Some(2));
}

/// Runs a longer history only when `EIDNARA_EVAL_S1_BUDGET_MS` grants a
/// budget.
#[test]
#[ignore = "S1 runs under an explicit budget: set EIDNARA_EVAL_S1_BUDGET_MS and run with --ignored"]
fn an_s1_campaign_runs_only_under_its_budget() {
    let Some(budget_ms) = budget(Scale::S1) else {
        return;
    };
    let run = campaign(Scale::S1, S1_AGED_MESSAGES, budget_ms);
    let report = &run.report;
    assert!(report.envelope.peaks.elapsed_ms <= budget_ms);
    assert_eq!(report.profile.name, "s1-surface1-raw");
    // Over 400 turns the daemon's summarizer fires more than at S0, and one
    // of its prompts draws a calibration example from the daemon's own seed
    // corpus that the secret scanner reads as a key, so the cassette refuses
    // the frame: the aged structured arm has no recording, its three samples
    // are skipped as refused, the aged arm's refusal rate is on the report,
    // and the refusal gate fails at the profile's ceiling of zero.
    assert!(run.aged.refused);
    assert_ne!(report.arm_rates["aged"].refusal_rate, "0");
    assert_eq!(report.arm_rates["fresh"].refusal_rate, "0");
    let ReportOutcome::Open { gated } = &report.outcome else {
        panic!("{:?}", report.outcome);
    };
    assert!(!gated.gates.redaction_refusals.passed, "{:?}", gated.gates);
}

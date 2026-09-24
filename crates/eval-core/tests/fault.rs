use std::collections::{BTreeMap, BTreeSet};

use eval_core::{
    APPLICATION_CRASH, Approval, ArtifactDeletionFaultKind, ArtifactGcFaultKind,
    ArtifactIngestFaultKind, BarrierReceipt, BarrierRefused, BatchFaultKind, CampaignProfile,
    ClaimBoundary, Coverage, CoverageRefused, Cut, CutCoverage, CutOutcome, DispatchFaultKind,
    EffectLedger, EffectOutcome, EffectRefused, EffectState, Envelope, EpisodeRefused, Expected,
    ExpectedRefusal, FAULT_REPORT_SCHEMA, FaultAction, FaultEpisode, FaultProfile, FaultReport,
    FaultReportError, FaultScope, Heal, HealthyCore, KillLabel, Lane, LaneProgress, LivenessBounds,
    LivenessRefused, LivenessReport, MaterializationFaultKind, PublicationFaultKind,
    RUN_PROFILE_SCHEMA, RecordedRefusal, ResourceLimits, RestoreFaultKind, RunProfile, SIGKILL,
    Scale, SearchEpisodeFault, StoreFamily, TEST_BINARY_CHILD, TaskBudgets, cut_receipts,
    parse_fault_report, validate_episodes,
};

const SUITE: &str = "crates/eval-core/tests/fault.rs::";

fn bounds() -> LivenessBounds {
    LivenessBounds {
        catch_up_episodes: 64,
        embedding_passes: 32,
        materialization_episodes: 16,
        reviewer_coordinator_passes: 8,
    }
}

/// An approved run profile whose liveness bounds are `bounds()` and whose
/// envelope is `limits()`; the only way to hold a `FaultProfile`.
fn run_profile() -> RunProfile {
    RunProfile {
        schema: RUN_PROFILE_SCHEMA.to_string(),
        name: "s0-fault-campaign".to_string(),
        scale: Scale::S0,
        worlds: 4,
        tasks_per_world: 3,
        max_events_per_log: 64,
        budgets: TaskBudgets {
            max_model_calls: 12,
            max_tool_calls: 40,
            max_tokens_in: 200_000,
            max_tokens_out: 32_000,
            hard_deadline_ms: 600_000,
            max_no_progress_iterations: 3,
        },
        envelope: limits(),
        indeterminate_ceiling: "0.1".to_string(),
        censoring_ceiling: "0.2".to_string(),
        redaction_refusal_ceiling: "0".to_string(),
        baseline_bounds: RunProfile::grounded_baseline_bounds(),
        statistics: CampaignProfile {
            noninferiority_margin: "0.02".to_string(),
            harm_bound: "0.1".to_string(),
            floor_threshold: "0.7".to_string(),
            miss_asymmetry_bound: "0.05".to_string(),
            liveness_bounds: bounds(),
        },
        approval: Some(Approval {
            approved_by: "maintainer".to_string(),
            approved_at_run_id: "ab".repeat(32),
        }),
    }
}

fn profile() -> FaultProfile {
    run_profile().fault_profile().unwrap()
}

fn limits() -> ResourceLimits {
    ResourceLimits {
        elapsed_ms: 1_800_000,
        store_bytes: 512 << 20,
        cassette_bytes: 64 << 20,
        artifact_bytes: 256 << 20,
        temp_roots: 4,
        retained_artifacts: 32,
        processes: 6,
    }
}

fn episode(id: &str, action: FaultAction) -> FaultEpisode {
    let heal = action.heal();
    let kill = action.is_kill().then(|| KillLabel {
        crash_model: APPLICATION_CRASH.to_string(),
        page_cache_intact: true,
        killed_process: TEST_BINARY_CHILD.to_string(),
    });
    FaultEpisode {
        id: id.to_string(),
        trigger_step: 3,
        scope: FaultScope {
            store: StoreFamily::SearchProjection,
            operation: "acknowledge".to_string(),
        },
        action,
        heal,
        layer_contract:
            "search_catchup::EpisodeFault: the acknowledgement commits, its reply is lost"
                .to_string(),
        kill,
    }
}

fn lost_ack() -> FaultAction {
    FaultAction::SearchEpisode {
        fault: SearchEpisodeFault::LoseAcknowledgementReply,
    }
}

fn r11() -> FaultAction {
    FaultAction::ExpectedRefusal {
        refusal: ExpectedRefusal::R11DeletionBearingCatchUp,
    }
}

fn kill() -> FaultAction {
    FaultAction::ProcessKill {
        cut: "acknowledged".to_string(),
    }
}

fn barrier(episode: &str) -> BarrierReceipt {
    BarrierReceipt {
        episode: episode.to_string(),
        cut: "acknowledged".to_string(),
        pid: 4242,
        line: "barrier acknowledged".to_string(),
        signal: SIGKILL,
    }
}

fn coverage(cuts: &[&str]) -> CutCoverage {
    let mut coverage = CutCoverage::default();
    for cut in cuts {
        coverage.declare(*cut);
        coverage.receipt(cut).unwrap();
    }
    coverage
}

fn lane(bound: u64, met_at: Option<u64>, holds: bool) -> LaneProgress {
    LaneProgress {
        bound,
        steps: bound,
        met_at,
        stalled_at: None,
        holds_at_bound: holds,
        fresh_commits: 4,
        blocked: None,
    }
}

fn liveness() -> LivenessReport {
    LivenessReport {
        core: HealthyCore {
            families: [StoreFamily::Memory, StoreFamily::SearchProjection]
                .into_iter()
                .collect(),
            lanes: [Lane::CatchUpEpisodes, Lane::EmbeddingPasses]
                .into_iter()
                .collect(),
        },
        outside_core: ["ingest-write".to_string()].into_iter().collect(),
        armed_at_bound: ["ingest-write".to_string()].into_iter().collect(),
        lanes: [
            (Lane::CatchUpEpisodes, lane(64, Some(3), true)),
            (Lane::EmbeddingPasses, lane(32, Some(2), true)),
        ]
        .into_iter()
        .collect(),
        permanent_stalls: vec![],
    }
}

fn report() -> FaultReport {
    let mut effects = EffectLedger::default();
    effects.attempt("ack:1");
    effects.lose_reply("ack:1", "lost-ack").unwrap();
    effects.read_back("ack:1", EffectState::Applied).unwrap();
    FaultReport {
        schema: FAULT_REPORT_SCHEMA.to_string(),
        eval_run_id: "ab".repeat(32),
        profile_digest: profile().digest().to_string(),
        claim_boundary: ClaimBoundary::pinned(),
        episodes: vec![
            episode("lost-ack", lost_ack()),
            episode("kill", kill()),
            episode("r11", r11()),
            FaultEpisode {
                scope: FaultScope {
                    store: StoreFamily::Kernel,
                    operation: "write".to_string(),
                },
                ..episode(
                    "ingest-write",
                    FaultAction::ArtifactIngest {
                        fault: ArtifactIngestFaultKind::Write,
                    },
                )
            },
        ],
        barriers: vec![barrier("kill")],
        cuts: cut_receipts(
            &[
                Cut::AfterAtomicTransition,
                Cut::AtQuiescence,
                Cut::AfterRecovery,
                Cut::AfterFaultPhase,
                Cut::EndOfRun,
            ],
            &[
                (Cut::AtQuiescence, 1),
                (Cut::AfterRecovery, 2),
                (Cut::AfterFaultPhase, 1),
                (Cut::EndOfRun, 1),
            ]
            .into_iter()
            .collect(),
        ),
        coverage: coverage(&["lost-ack", "kill", "ingest-write", "acknowledged", "r11"]),
        effects,
        expected_refusals: vec![RecordedRefusal {
            episode: "r11".to_string(),
            refusal: ExpectedRefusal::R11DeletionBearingCatchUp,
            production_error: "DeletionUnpropagated { commit_seq: 7 }".to_string(),
        }],
        safety_checks_while_armed: 5,
        liveness: Some(liveness()),
        markers: BTreeSet::new(),
        envelope: {
            let mut envelope = Envelope::new(limits());
            envelope.peaks.processes = 1;
            envelope
        },
    }
}

#[test]
fn every_episode_is_a_named_action_with_the_heal_its_seam_permits() {
    let ok = episode("lost-ack", lost_ack());
    ok.validate().unwrap();
    assert_eq!(ok.heal, Heal::Consumed);
    assert_eq!(
        FaultAction::HeldPublication.heal(),
        Heal::Released,
        "a gate is released"
    );
    assert_eq!(FaultAction::ExternalLockHolder.heal(), Heal::Released);
    assert_eq!(FaultAction::CorruptQuiescentFile.heal(), Heal::Reopen);
    assert_eq!(
        FaultAction::ArtifactIngest {
            fault: ArtifactIngestFaultKind::AfterDirectorySync
        }
        .heal(),
        Heal::Reopen,
        "an EIO in the CAS latches ingestion closed until reopen"
    );
    for fault in [
        ArtifactIngestFaultKind::ReservationCommit,
        ArtifactIngestFaultKind::AfterEvents,
    ] {
        let action = FaultAction::ArtifactIngest { fault };
        assert_eq!(
            action.heal(),
            Heal::Consumed,
            "{fault:?} fails a transaction without latching, so the store stays usable"
        );
        let mut consumed = episode("ingest-transaction", action);
        consumed.scope.store = StoreFamily::Kernel;
        consumed.heal = Heal::Consumed;
        consumed.validate().unwrap();
    }
    for fault in [
        ArtifactIngestFaultKind::Write,
        ArtifactIngestFaultKind::FileSync,
        ArtifactIngestFaultKind::Rename,
        ArtifactIngestFaultKind::TakeoverBeforeCleanupUnlink,
    ] {
        assert_eq!(
            FaultAction::ArtifactIngest { fault }.heal(),
            Heal::Reopen,
            "{fault:?} latches ingestion closed"
        );
    }
    assert_eq!(
        FaultAction::ArtifactDeletion {
            fault: ArtifactDeletionFaultKind::IntentStorageExhausted
        }
        .heal(),
        Heal::Consumed,
        "ENOSPC does not latch"
    );
    assert_eq!(
        FaultAction::ArtifactDeletion {
            fault: ArtifactDeletionFaultKind::Unlink
        }
        .heal(),
        Heal::Reopen
    );
    assert_eq!(
        r11().heal(),
        Heal::Reopen,
        "the reopen's projection rebuild clears a deletion-bearing stall"
    );
    assert_eq!(
        FaultAction::ExpectedRefusal {
            refusal: ExpectedRefusal::R24ReceiptQuotaExhausted
        }
        .heal(),
        Heal::Permanent,
        "retained receipt charges refuse admission for the rest of the store incarnation"
    );
    let mut healed_wrong = ok.clone();
    healed_wrong.heal = Heal::Reopen;
    assert_eq!(
        healed_wrong.validate(),
        Err(EpisodeRefused::HealMismatch {
            id: "lost-ack".to_string(),
            declared: Heal::Reopen,
            required: Heal::Consumed,
        })
    );
    let mut silent = ok.clone();
    silent.layer_contract = "  ".to_string();
    assert!(matches!(
        silent.validate(),
        Err(EpisodeRefused::EmptyLayerContract { .. })
    ));
    let mut labelled = ok.clone();
    labelled.kill = Some(KillLabel {
        crash_model: APPLICATION_CRASH.to_string(),
        page_cache_intact: true,
        killed_process: TEST_BINARY_CHILD.to_string(),
    });
    assert!(matches!(
        labelled.validate(),
        Err(EpisodeRefused::KillLabelOnNonKill { .. })
    ));
    assert_eq!(
        validate_episodes(&[ok.clone(), ok.clone()]),
        Err(EpisodeRefused::DuplicateEpisode {
            id: "lost-ack".to_string()
        })
    );
    let value = serde_json::to_value(&ok).unwrap();
    assert_eq!(value["action"]["kind"], "search_episode");
    assert_eq!(value["action"]["fault"], "lose_acknowledgement_reply");
    assert_eq!(serde_json::from_value::<FaultEpisode>(value).unwrap(), ok);
}

#[test]
fn a_power_loss_label_and_a_host_kill_are_refused() {
    let mut coverage = Coverage::default();
    let ok = episode("kill", kill());
    ok.validate().unwrap();
    for (crash_model, page_cache_intact) in [
        ("power_loss", false),
        ("torn_write", true),
        ("unsynced_reorder", true),
        (APPLICATION_CRASH, false),
    ] {
        let mut wrong = ok.clone();
        wrong.kill = Some(KillLabel {
            crash_model: crash_model.to_string(),
            page_cache_intact,
            killed_process: TEST_BINARY_CHILD.to_string(),
        });
        assert_eq!(
            wrong.validate(),
            Err(EpisodeRefused::CrashModelNotProved {
                id: "kill".to_string(),
                crash_model: crash_model.to_string(),
            })
        );
    }
    let mut host = ok.clone();
    host.kill = Some(KillLabel {
        crash_model: APPLICATION_CRASH.to_string(),
        page_cache_intact: true,
        killed_process: "eidnara_host".to_string(),
    });
    assert_eq!(
        host.validate(),
        Err(EpisodeRefused::KilledProcessNotProved {
            id: "kill".to_string(),
            killed_process: "eidnara_host".to_string(),
        })
    );
    let mut unlabelled = ok;
    unlabelled.kill = None;
    assert!(matches!(
        unlabelled.validate(),
        Err(EpisodeRefused::KillLabelMissing { .. })
    ));
    coverage.record("flt_crash_model_label_refused").unwrap();
    assert!(
        barrier("kill").validate().is_ok(),
        "a barrier line ending in the cut, read from a signalled child, is a receipt"
    );
    let mut other_cut = barrier("kill");
    other_cut.line = "barrier staged".to_string();
    assert!(matches!(
        other_cut.validate(),
        Err(BarrierRefused::LineDoesNotNameCut { .. })
    ));
    let mut suffix = barrier("kill");
    suffix.line = "barrier unacknowledged".to_string();
    assert!(
        matches!(
            suffix.validate(),
            Err(BarrierRefused::LineDoesNotNameCut { .. })
        ),
        "the cut is the line's last token, not a suffix of it"
    );
    let mut empty = barrier("kill");
    empty.cut = String::new();
    assert!(
        matches!(
            empty.validate(),
            Err(BarrierRefused::LineDoesNotNameCut { .. })
        ),
        "no line names an empty cut"
    );
    let mut exited = barrier("kill");
    exited.signal = 0;
    assert!(matches!(
        exited.validate(),
        Err(BarrierRefused::ExitedWithStatus { .. })
    ));
}

#[test]
fn a_missing_receipt_is_incomplete_coverage_not_pass() {
    let mut coverage = Coverage::default();
    let mut cuts = CutCoverage::default();
    cuts.declare("staged");
    cuts.declare("released");
    cuts.declare("acknowledged");
    cuts.receipt("staged").unwrap();
    cuts.receipt("staged").unwrap();
    cuts.receipt("acknowledged").unwrap();
    assert_eq!(
        cuts.verdict(),
        Err(CoverageRefused::IncompleteCoverage {
            missing: ["released".to_string()].into_iter().collect(),
        })
    );
    assert_eq!(
        cuts.receipt("unlink"),
        Err(CoverageRefused::UndeclaredCut {
            cut: "unlink".to_string()
        }),
        "a receipt for a cut nobody declared is not coverage of anything"
    );
    cuts.receipt("released").unwrap();
    cuts.verdict().unwrap();
    assert_eq!(cuts.receipted["staged"], 2);
    coverage
        .record("flt_incomplete_coverage_named_not_pass")
        .unwrap();

    let receipts = cut_receipts(
        &[Cut::AtQuiescence, Cut::AfterRecovery, Cut::EndOfRun],
        &[(Cut::AtQuiescence, 1), (Cut::EndOfRun, 3)]
            .into_iter()
            .collect(),
    );
    assert_eq!(
        receipts
            .iter()
            .map(|r| (r.cut, r.outcome))
            .collect::<Vec<_>>(),
        vec![
            (Cut::AtQuiescence, CutOutcome::Reached),
            (Cut::AfterRecovery, CutOutcome::NotReached),
            (Cut::EndOfRun, CutOutcome::Reached),
        ]
    );
}

#[test]
fn a_lost_reply_is_unknown_over_an_admissible_set_until_a_read_back_names_one_state() {
    let mut ledger = EffectLedger::default();
    ledger.attempt("commit:5");
    ledger.acknowledge("commit:5").unwrap();
    ledger.attempt("commit:6");
    ledger.lose_reply("commit:6", "lost-ack").unwrap();
    let lost = &ledger.effects["commit:6"];
    assert_eq!(lost.outcome, EffectOutcome::Unknown);
    assert_eq!(
        lost.expected,
        Expected::OneOf {
            states: [EffectState::Applied, EffectState::NotApplied]
                .into_iter()
                .collect(),
        }
    );
    assert_eq!(
        ledger.unknown(),
        ["commit:6".to_string()].into_iter().collect()
    );
    ledger.validate().unwrap();

    let mut applied = ledger.clone();
    applied.read_back("commit:6", EffectState::Applied).unwrap();
    let e = &applied.effects["commit:6"];
    assert_eq!(e.outcome, EffectOutcome::Applied);
    assert_eq!(
        e.expected,
        Expected::Exactly {
            state: EffectState::Applied
        }
    );
    assert_eq!((e.attempted, e.observed, e.acknowledged), (1, 1, 0));
    applied.validate().unwrap();
    assert!(applied.unknown().is_empty());

    let mut not_applied = ledger.clone();
    not_applied
        .read_back("commit:6", EffectState::NotApplied)
        .unwrap();
    let e = &not_applied.effects["commit:6"];
    assert_eq!(e.outcome, EffectOutcome::NotApplied);
    assert_eq!((e.attempted, e.observed, e.acknowledged), (1, 0, 0));
    not_applied.validate().unwrap();

    let before = ledger.clone();
    assert_eq!(
        ledger.read_back("commit:5", EffectState::NotApplied),
        Err(EffectRefused::ReadBackNotAdmissible {
            identity: "commit:5".to_string(),
            state: EffectState::NotApplied,
        }),
        "an acknowledged effect read back as not applied is a lost acknowledged write"
    );
    assert_eq!(ledger, before, "a refused read-back changes nothing");
    let mut acked_then_lost = ledger.clone();
    acked_then_lost.lose_reply("commit:5", "lost-ack").unwrap();
    assert!(matches!(
        acked_then_lost.read_back("commit:5", EffectState::NotApplied),
        Err(EffectRefused::ReadBackNotAdmissible { .. })
    ));
    ledger.attempt("commit:7");
    assert!(
        matches!(
            ledger.read_back("commit:7", EffectState::NotApplied),
            Err(EffectRefused::ReadBackNotAdmissible { .. })
        ),
        "a reply that was not lost admits only the applied state"
    );

    assert_eq!(
        ledger.observe("commit:9"),
        Err(EffectRefused::UnknownIdentity {
            identity: "commit:9".to_string()
        })
    );
}

#[test]
fn a_premature_success_fixture_is_refused() {
    let mut coverage = Coverage::default();
    let mut ledger = EffectLedger::default();
    ledger.attempt("ack:2");
    ledger.lose_reply("ack:2", "lost-ack").unwrap();
    ledger.effects.get_mut("ack:2").unwrap().outcome = EffectOutcome::Applied;
    assert_eq!(
        ledger.validate(),
        Err(EffectRefused::PrematureSuccess {
            identity: "ack:2".to_string()
        })
    );
    let mut collapsed = EffectLedger::default();
    collapsed.attempt("ack:3");
    collapsed.lose_reply("ack:3", "lost-ack").unwrap();
    collapsed.effects.get_mut("ack:3").unwrap().expected = Expected::Exactly {
        state: EffectState::NotApplied,
    };
    assert_eq!(
        collapsed.validate(),
        Err(EffectRefused::ExpectationCollapsedWithoutReadBack {
            identity: "ack:3".to_string()
        })
    );
    let mut rewritten = EffectLedger::default();
    rewritten.attempt("ack:6");
    rewritten.acknowledge("ack:6").unwrap();
    let e = rewritten.effects.get_mut("ack:6").unwrap();
    e.read_back = true;
    e.expected = Expected::Exactly {
        state: EffectState::NotApplied,
    };
    e.outcome = EffectOutcome::NotApplied;
    assert_eq!(
        rewritten.validate(),
        Err(EffectRefused::ReadBackNotAdmissible {
            identity: "ack:6".to_string(),
            state: EffectState::NotApplied,
        }),
        "a parsed ledger cannot record an acknowledged effect as not applied"
    );
    let mut inflated = EffectLedger::default();
    inflated.attempt("ack:4");
    inflated.acknowledge("ack:4").unwrap();
    inflated.acknowledge("ack:4").unwrap();
    assert_eq!(
        inflated.validate(),
        Err(EffectRefused::BoundsViolated {
            identity: "ack:4".to_string(),
            attempted: 1,
            observed: 2,
            acknowledged: 2,
        }),
        "acknowledged <= observed <= attempted is checked per identity, never inferred from totals"
    );
    let mut aggregate_hides_it = inflated;
    aggregate_hides_it.attempt("ack:5");
    aggregate_hides_it.attempt("ack:5");
    aggregate_hides_it.attempt("ack:5");
    let totals = aggregate_hides_it
        .effects
        .values()
        .fold((0, 0, 0), |(a, o, k), e| {
            (a + e.attempted, o + e.observed, k + e.acknowledged)
        });
    assert!(totals.2 <= totals.1 && totals.1 <= totals.0);
    assert!(matches!(
        aggregate_hides_it.validate(),
        Err(EffectRefused::BoundsViolated { .. })
    ));
    coverage
        .record("flt_premature_success_fixture_refused")
        .unwrap();
}

#[test]
fn liveness_is_unmet_at_the_bound_or_when_a_fault_healed() {
    let mut coverage = Coverage::default();
    let ok = liveness();
    ok.verdict(&bounds()).unwrap();

    let mut healed = ok.clone();
    healed.armed_at_bound.clear();
    assert_eq!(
        healed.verdict(&bounds()),
        Err(LivenessRefused::FaultHealed {
            episode: "ingest-write".to_string()
        })
    );
    let mut inside = ok.clone();
    inside
        .armed_at_bound
        .insert("catch-up-lost-ack".to_string());
    assert_eq!(
        inside.verdict(&bounds()),
        Err(LivenessRefused::ArmedInsideCore {
            episode: "catch-up-lost-ack".to_string()
        })
    );
    let mut never = ok.clone();
    never.lanes.insert(
        Lane::CatchUpEpisodes,
        LaneProgress {
            blocked: Some("DeletionUnpropagated { commit_seq: 4 }".to_string()),
            ..lane(64, None, false)
        },
    );
    assert_eq!(
        never.verdict(&bounds()),
        Err(LivenessRefused::LivenessUnmet {
            lane: Lane::CatchUpEpisodes,
            progress_at_bound: 64,
            blocked: Some("DeletionUnpropagated { commit_seq: 4 }".to_string()),
        })
    );
    let mut transient = ok.clone();
    transient
        .lanes
        .insert(Lane::EmbeddingPasses, lane(32, Some(2), false));
    assert!(matches!(
        transient.verdict(&bounds()),
        Err(LivenessRefused::LivenessUnmet {
            lane: Lane::EmbeddingPasses,
            ..
        })
    ));
    let mut stalled = ok.clone();
    stalled
        .lanes
        .get_mut(&Lane::CatchUpEpisodes)
        .unwrap()
        .stalled_at = Some(40);
    assert!(
        matches!(
            stalled.verdict(&bounds()),
            Err(LivenessRefused::LivenessUnmet {
                lane: Lane::CatchUpEpisodes,
                ..
            })
        ),
        "a predicate that held, failed, and held again at the bound is not sustained progress"
    );
    let mut idle = ok.clone();
    idle.lanes
        .get_mut(&Lane::EmbeddingPasses)
        .unwrap()
        .fresh_commits = 0;
    assert!(
        matches!(
            idle.verdict(&bounds()),
            Err(LivenessRefused::LivenessUnmet {
                lane: Lane::EmbeddingPasses,
                ..
            })
        ),
        "a lane fed no fresh work meets its predicate trivially and proves no progress"
    );
    let mut unarmed = ok.clone();
    unarmed.outside_core.clear();
    unarmed.armed_at_bound.clear();
    assert_eq!(
        unarmed.verdict(&bounds()),
        Err(LivenessRefused::NoOutsideCoreFault),
        "the liveness mode runs with outside-core faults armed, so none is no run"
    );
    let mut short = ok.clone();
    short.lanes.get_mut(&Lane::CatchUpEpisodes).unwrap().steps = 10;
    assert!(
        matches!(
            short.verdict(&bounds()),
            Err(LivenessRefused::LivenessUnmet { .. })
        ),
        "stopping before the bound never shows the predicate held at it"
    );
    let mut undriven = ok.clone();
    undriven.lanes.remove(&Lane::EmbeddingPasses);
    assert_eq!(
        undriven.verdict(&bounds()),
        Err(LivenessRefused::LaneNotDriven {
            lane: Lane::EmbeddingPasses
        })
    );
    let mut fitted = ok;
    fitted.lanes.get_mut(&Lane::CatchUpEpisodes).unwrap().bound = 3;
    fitted.lanes.get_mut(&Lane::CatchUpEpisodes).unwrap().steps = 3;
    assert_eq!(
        fitted.verdict(&bounds()),
        Err(LivenessRefused::BoundMismatch {
            lane: Lane::CatchUpEpisodes,
            declared: 3,
            profile: 64,
        }),
        "a bound fitted to the observed progress is not the profile's bound"
    );
    coverage
        .record("flt_liveness_unmet_named_at_bound")
        .unwrap();
    let mut zero = bounds();
    zero.reviewer_coordinator_passes = 0;
    assert_eq!(Lane::ReviewerCoordinatorPasses.bound(&zero), 0);
}

#[test]
fn a_fault_report_round_trips_and_refuses_what_it_cannot_prove() {
    let report = report();
    let value = report.serialize(&profile()).unwrap();
    assert_eq!(parse_fault_report(&value, &profile()).unwrap(), report);
    let digest = FaultReport::result_digest(&value).unwrap();
    let mut other_pid = value.clone();
    other_pid["barriers"][0]["pid"] = serde_json::json!(1);
    other_pid["envelope"]["peaks"]["elapsed_ms"] = serde_json::json!(99);
    assert_eq!(FaultReport::result_digest(&other_pid).unwrap(), digest);
    let mut other_outcome = value.clone();
    other_outcome["effects"]["effects"]["ack:1"]["outcome"] = serde_json::json!("unknown");
    assert_ne!(FaultReport::result_digest(&other_outcome).unwrap(), digest);

    let mut wrong_schema = report.clone();
    wrong_schema.schema = "eval-suite-c-fault-report/v0".to_string();
    assert!(matches!(
        wrong_schema.validate(&profile()),
        Err(FaultReportError::SchemaMismatch { .. })
    ));
    let mut no_barrier = report.clone();
    no_barrier.barriers.clear();
    assert_eq!(
        no_barrier.validate(&profile()),
        Err(FaultReportError::KillWithoutBarrier {
            episode: "kill".to_string(),
            cut: "acknowledged".to_string(),
        })
    );
    let mut wrong_cut = report.clone();
    wrong_cut.barriers = vec![BarrierReceipt {
        cut: "staged".to_string(),
        line: "barrier staged".to_string(),
        ..barrier("kill")
    }];
    assert_eq!(
        wrong_cut.validate(&profile()),
        Err(FaultReportError::KillWithoutBarrier {
            episode: "kill".to_string(),
            cut: "acknowledged".to_string(),
        }),
        "a barrier from another cut does not place the kill at its declared cut"
    );
    let mut ghost = report.clone();
    let live = ghost.liveness.as_mut().unwrap();
    live.outside_core.insert("ghost".to_string());
    live.armed_at_bound.insert("ghost".to_string());
    assert_eq!(
        ghost.validate(&profile()),
        Err(FaultReportError::UnknownEpisode {
            episode: "ghost".to_string()
        }),
        "an armed outside-core fault the campaign never ran is not evidence"
    );
    let mut unreceipted = report.clone();
    unreceipted.coverage.declare("unlink");
    assert!(matches!(
        unreceipted.validate(&profile()),
        Err(FaultReportError::Coverage(
            CoverageRefused::IncompleteCoverage { .. }
        ))
    ));
    let mut unsafe_run = report.clone();
    unsafe_run.safety_checks_while_armed = 0;
    assert_eq!(
        unsafe_run.validate(&profile()),
        Err(FaultReportError::SafetyNeverChecked)
    );
    let mut premature = report.clone();
    premature.effects.attempt("ack:9");
    premature.effects.lose_reply("ack:9", "lost-ack").unwrap();
    premature.effects.effects.get_mut("ack:9").unwrap().outcome = EffectOutcome::Applied;
    assert!(matches!(
        premature.validate(&profile()),
        Err(FaultReportError::Effect(
            EffectRefused::PrematureSuccess { .. }
        ))
    ));
    let mut borrowed = report.clone();
    borrowed.episodes[2].action = FaultAction::ArtifactDeletion {
        fault: ArtifactDeletionFaultKind::AfterCommit,
    };
    borrowed.episodes[2].heal = Heal::Consumed;
    borrowed.episodes[2].scope.store = StoreFamily::Kernel;
    assert_eq!(
        borrowed.validate(&profile()),
        Err(FaultReportError::RefusalNotDeclared {
            episode: "r11".to_string()
        }),
        "a refusal recorded against an injected fault's label claims a fault that never ran"
    );
    let mut mislabelled = report.clone();
    mislabelled.expected_refusals[0].refusal = ExpectedRefusal::R24ReceiptQuotaExhausted;
    mislabelled.expected_refusals[0].production_error = "MetadataQuota".to_string();
    assert_eq!(
        mislabelled.validate(&profile()),
        Err(FaultReportError::RefusalNotDeclared {
            episode: "r11".to_string()
        }),
        "an evidenced refusal recorded against an episode declaring another refusal"
    );
    let mut stalled_elsewhere = report.clone();
    stalled_elsewhere
        .liveness
        .as_mut()
        .unwrap()
        .permanent_stalls
        .push(RecordedRefusal {
            episode: "lost-ack".to_string(),
            refusal: ExpectedRefusal::R11DeletionBearingCatchUp,
            production_error: "DeletionUnpropagated { commit_seq: 9 }".to_string(),
        });
    assert_eq!(
        stalled_elsewhere.validate(&profile()),
        Err(FaultReportError::RefusalNotDeclared {
            episode: "lost-ack".to_string()
        }),
        "a permanent stall recorded against an injected fault's episode claims a refusal that episode never declared"
    );
    let mut stalled_only = report.clone();
    let stall = stalled_only.expected_refusals.remove(0);
    stalled_only
        .liveness
        .as_mut()
        .unwrap()
        .permanent_stalls
        .push(stall);
    stalled_only.validate(&profile()).expect(
        "a permanent stall recorded against its expected_refusal episode is the record that episode needs",
    );
    let mut unrecorded = report.clone();
    unrecorded.expected_refusals.clear();
    assert_eq!(
        unrecorded.validate(&profile()),
        Err(FaultReportError::RefusalNotRecorded {
            episode: "r11".to_string()
        }),
        "an expected-refusal episode with no recorded production error claims a refusal the run never observed"
    );
    let mut extra = value.clone();
    extra["surprise"] = serde_json::json!(1);
    assert!(matches!(
        parse_fault_report(&extra, &profile()),
        Err(FaultReportError::Shape(_))
    ));
    let mut liveness_less = report;
    liveness_less.liveness = None;
    liveness_less.validate(&profile()).unwrap();
    let _ = (
        PublicationFaultKind::LoseLocalCommit,
        BTreeMap::<String, u64>::new(),
    );
}

#[test]
fn a_parsed_report_cannot_claim_what_no_run_recorded() {
    let ok = report();
    let b = bounds();
    let p = profile();

    let mut boundary = ok.clone();
    boundary.claim_boundary.exclusions.clear();
    assert_eq!(
        boundary.validate(&p),
        Err(FaultReportError::ClaimBoundaryMismatch)
    );
    let mut over = ok.clone();
    over.envelope.peaks.processes = limits().processes + 1;
    assert!(matches!(
        over.validate(&p),
        Err(FaultReportError::EnvelopeExceeded(_))
    ));
    let mut no_fault = ok.clone();
    no_fault.episodes.clear();
    no_fault.barriers.clear();
    no_fault.coverage = CutCoverage::default();
    no_fault.liveness = None;
    assert_eq!(
        no_fault.validate(&p),
        Err(FaultReportError::NoEpisode),
        "nothing was armed, so no safety check ran while a fault was"
    );
    let mut refusal_only = ok.clone();
    refusal_only.episodes.retain(|e| e.id == "r11");
    refusal_only.barriers.clear();
    refusal_only.coverage = coverage(&["r11"]);
    refusal_only.effects = EffectLedger::default();
    refusal_only.liveness = None;
    assert_eq!(
        refusal_only.validate(&p),
        Err(FaultReportError::NoEpisode),
        "an expected refusal injects no fault, so a report of refusals alone armed nothing"
    );
    let mut invented = ok.clone();
    invented.markers.insert("flt_never_registered".to_string());
    assert_eq!(
        invented.validate(&p),
        Err(FaultReportError::UnregisteredMarker {
            marker: "flt_never_registered".to_string()
        })
    );
    let mut in_core = ok.clone();
    in_core.episodes[3].action = FaultAction::ExternalLockHolder;
    in_core.episodes[3].heal = Heal::Released;
    in_core.episodes[3].scope.store = StoreFamily::SearchProjection;
    assert_eq!(
        in_core.validate(&p),
        Err(FaultReportError::CoreFamilyFaulted {
            episode: "ingest-write".to_string(),
            store: StoreFamily::SearchProjection,
        }),
        "a fault scoped to a healthy-core family is not outside the core"
    );
    let mut consumed = ok.clone();
    consumed.episodes[3].action = FaultAction::ArtifactIngest {
        fault: ArtifactIngestFaultKind::ReservationCommit,
    };
    consumed.episodes[3].heal = Heal::Consumed;
    assert_eq!(
        consumed.validate(&p),
        Err(FaultReportError::ConsumedFaultArmed {
            episode: "ingest-write".to_string()
        }),
        "a one-shot fault is consumed or never fired; neither is armed at the bound"
    );
    let mut refusal_armed = ok.clone();
    {
        let live = refusal_armed.liveness.as_mut().unwrap();
        live.core.families = [StoreFamily::Kernel].into_iter().collect();
        live.outside_core = ["r11".to_string()].into_iter().collect();
        live.armed_at_bound = ["r11".to_string()].into_iter().collect();
    }
    assert_eq!(
        refusal_armed.validate(&p),
        Err(FaultReportError::RefusalArmed {
            episode: "r11".to_string()
        }),
        "an expected refusal injects no fault, so it is not an armed outside-core fault"
    );

    let mut unnamed = ok.episodes[0].clone();
    unnamed.id = " ".to_string();
    assert_eq!(
        unnamed.validate(),
        Err(EpisodeRefused::EmptyId),
        "an episode nobody can name is not a named, scoped action"
    );
    let mut unscoped = ok.episodes[0].clone();
    unscoped.scope.operation = String::new();
    assert_eq!(
        unscoped.validate(),
        Err(EpisodeRefused::EmptyOperation {
            id: "lost-ack".to_string()
        })
    );

    let mut stray = coverage(&["kill"]);
    stray.receipted.insert("unlink".to_string(), 1);
    assert_eq!(
        stray.verdict(),
        Err(CoverageRefused::UndeclaredCut {
            cut: "unlink".to_string()
        }),
        "a parsed receipt for an undeclared cut is the receipt the API refuses"
    );

    let mut claimed = ok.effects.clone();
    let effect = claimed.effects.get_mut("ack:1").unwrap();
    effect.observed = 0;
    effect.lost_by.clear();
    effect.read_back = false;
    effect.expected = Expected::Exactly {
        state: EffectState::NotApplied,
    };
    effect.outcome = EffectOutcome::NotApplied;
    assert_eq!(
        claimed.validate(),
        Err(EffectRefused::OutcomeNotDerived {
            identity: "ack:1".to_string()
        }),
        "a reply that was not lost derives applied; nothing else was observed"
    );
    let mut contradicted = ok.effects.clone();
    let effect = contradicted.effects.get_mut("ack:1").unwrap();
    effect.observed = 0;
    effect.expected = Expected::Exactly {
        state: EffectState::NotApplied,
    };
    assert_eq!(
        contradicted.validate(),
        Err(EffectRefused::OutcomeNotDerived {
            identity: "ack:1".to_string()
        }),
        "an outcome must be the state its expectation names"
    );

    let mut laneless = liveness();
    laneless.core.lanes.clear();
    assert_eq!(
        laneless.verdict(&b),
        Err(LivenessRefused::EmptyHealthyCore),
        "a core with no lane to drive proves no liveness"
    );
    let mut familyless = liveness();
    familyless.core.families.clear();
    assert_eq!(
        familyless.verdict(&b),
        Err(LivenessRefused::EmptyHealthyCore)
    );

    let mut overcounted = EffectLedger::default();
    overcounted.attempt("dup");
    overcounted.observe("dup").unwrap();
    overcounted.observe("dup").unwrap();
    overcounted.lose_reply("dup", "lost-ack").unwrap();
    overcounted.read_back("dup", EffectState::Applied).unwrap();
    assert_eq!(overcounted.effects["dup"].observed, 2);
    assert!(
        matches!(
            overcounted.validate(),
            Err(EffectRefused::BoundsViolated {
                attempted: 1,
                observed: 2,
                ..
            })
        ),
        "an applied read-back adds evidence; it never erases an over-count"
    );

    let mut no_run = ok.clone();
    no_run.eval_run_id = String::new();
    assert_eq!(
        no_run.validate(&p),
        Err(FaultReportError::MalformedDigest {
            field: "eval_run_id"
        })
    );
    let mut no_profile = ok.clone();
    no_profile.profile_digest = "ZZ".repeat(32);
    assert_eq!(
        no_profile.validate(&p),
        Err(FaultReportError::MalformedDigest {
            field: "profile_digest"
        })
    );

    let materializer = FaultAction::ClaimMaterialization {
        fault: MaterializationFaultKind::SkipAcknowledgement,
    };
    assert_eq!(materializer.heal(), Heal::Consumed);
    assert_eq!(materializer.family(), Some(StoreFamily::Kernel));
    assert_eq!(
        serde_json::to_value(&materializer).unwrap(),
        serde_json::json!({"kind": "claim_materialization", "fault": "skip_acknowledgement"})
    );
    let mut mislabelled = ok.episodes[3].clone();
    mislabelled.scope.store = StoreFamily::Memory;
    assert_eq!(
        mislabelled.validate(),
        Err(EpisodeRefused::ScopeMismatch {
            id: "ingest-write".to_string(),
            declared: StoreFamily::Memory,
            required: StoreFamily::Kernel,
        }),
        "a CAS fault is a kernel fault whatever the episode says"
    );
    assert_eq!(FaultAction::ExternalLockHolder.family(), None);

    for signal in [-1, 15, 999] {
        let mut not_killed = barrier("kill");
        not_killed.signal = signal;
        assert_eq!(
            not_killed.validate(),
            Err(BarrierRefused::NotSigkill {
                episode: "kill".to_string(),
                signal,
            }),
            "only SIGKILL is the application crash the label describes"
        );
    }

    let mut ghost_refusal = ok.clone();
    ghost_refusal.expected_refusals[0].episode = "ghost".to_string();
    assert_eq!(
        ghost_refusal.validate(&p),
        Err(FaultReportError::UnknownEpisode {
            episode: "ghost".to_string()
        })
    );
    let mut unevidenced = ok.clone();
    unevidenced.expected_refusals[0].production_error = String::new();
    assert_eq!(
        unevidenced.validate(&p),
        Err(FaultReportError::RefusalNotEvidenced {
            episode: "r11".to_string(),
            refusal: ExpectedRefusal::R11DeletionBearingCatchUp,
        })
    );
    let mut stalled_ghost = ok.clone();
    stalled_ghost
        .liveness
        .as_mut()
        .unwrap()
        .permanent_stalls
        .push(RecordedRefusal {
            episode: "ghost".to_string(),
            refusal: ExpectedRefusal::R24ReceiptQuotaExhausted,
            production_error: "MetadataQuota".to_string(),
        });
    assert_eq!(
        stalled_ghost.validate(&p),
        Err(FaultReportError::UnknownEpisode {
            episode: "ghost".to_string()
        }),
        "a permanent stall is a recorded refusal too"
    );

    let mut orphan_barrier = ok.clone();
    orphan_barrier.barriers.push(barrier("ingest-write"));
    assert_eq!(
        orphan_barrier.validate(&p),
        Err(FaultReportError::BarrierWithoutKill {
            episode: "ingest-write".to_string(),
            cut: "acknowledged".to_string(),
        }),
        "a barrier claims a kill; only a kill episode at that cut backs it"
    );
    let mut other_cut = ok.clone();
    other_cut.barriers.push(BarrierReceipt {
        cut: "staged".to_string(),
        line: "barrier staged".to_string(),
        ..barrier("kill")
    });
    assert_eq!(
        other_cut.validate(&p),
        Err(FaultReportError::BarrierWithoutKill {
            episode: "kill".to_string(),
            cut: "staged".to_string(),
        })
    );
    let mut twice = ok.clone();
    twice.cuts.push(eval_core::CutReceipt {
        cut: Cut::AtQuiescence,
        outcome: CutOutcome::NotReached,
    });
    assert_eq!(
        twice.validate(&p),
        Err(FaultReportError::DuplicateCut {
            cut: Cut::AtQuiescence
        }),
        "two outcomes for one checkpoint is no outcome"
    );
    let mut untried = ok.effects.clone();
    let effect = untried.effects.get_mut("ack:1").unwrap();
    effect.attempted = 0;
    effect.observed = 0;
    effect.lost_by.clear();
    effect.read_back = false;
    assert_eq!(
        untried.validate(),
        Err(EffectRefused::NeverAttempted {
            identity: "ack:1".to_string()
        })
    );
    let mut unsafe_int = ok.clone();
    unsafe_int.safety_checks_while_armed = 9_007_199_254_740_992;
    assert!(
        matches!(
            unsafe_int.serialize(&p),
            Err(FaultReportError::NotCanonical(_))
        ),
        "a report the result digest refuses is not serialized as valid"
    );
    let mut unsafe_value = ok.serialize(&p).unwrap();
    unsafe_value["safety_checks_while_armed"] = serde_json::json!(9_007_199_254_740_992u64);
    assert!(matches!(
        parse_fault_report(&unsafe_value, &p),
        Err(FaultReportError::NotCanonical(_))
    ));
    let mut met_but_blocked = liveness();
    met_but_blocked
        .lanes
        .get_mut(&Lane::CatchUpEpisodes)
        .unwrap()
        .blocked = Some("DeletionUnpropagated { commit_seq: 9 }".to_string());
    assert!(
        matches!(
            met_but_blocked.verdict(&b),
            Err(LivenessRefused::LivenessUnmet {
                lane: Lane::CatchUpEpisodes,
                blocked: Some(_),
                ..
            })
        ),
        "a lane that records the block that stopped it did not meet its bound"
    );

    let mut raised = ok.clone();
    raised.envelope.peaks.processes = limits().processes + 1;
    raised.envelope.bounds.processes = limits().processes + 2;
    assert_eq!(
        raised.validate(&p),
        Err(FaultReportError::EnvelopeDisagreesWithProfile),
        "a report may not widen the envelope its profile approved"
    );
    let mut seen_yet_absent = ok.effects.clone();
    let effect = seen_yet_absent.effects.get_mut("ack:1").unwrap();
    effect.expected = Expected::Exactly {
        state: EffectState::NotApplied,
    };
    effect.outcome = EffectOutcome::NotApplied;
    assert_eq!(
        seen_yet_absent.validate(),
        Err(EffectRefused::ReadBackNotAdmissible {
            identity: "ack:1".to_string(),
            state: EffectState::NotApplied,
        }),
        "an observed effect was applied, whatever a read-back says"
    );
    let mut observed = EffectLedger::default();
    observed.attempt("seen");
    observed.observe("seen").unwrap();
    observed.lose_reply("seen", "lost-ack").unwrap();
    assert!(matches!(
        observed.read_back("seen", EffectState::NotApplied),
        Err(EffectRefused::ReadBackNotAdmissible { .. })
    ));
    let mut unledgered = ok.clone();
    unledgered.effects = EffectLedger::default();
    assert_eq!(
        unledgered.validate(&p),
        Err(FaultReportError::LostReplyUnrecorded {
            episode: "lost-ack".to_string()
        }),
        "a lost reply nobody ledgered left no evidence of what it did"
    );
    let mut misattributed = ok.clone();
    misattributed.effects = EffectLedger::default();
    misattributed.effects.attempt("unrelated");
    misattributed
        .effects
        .lose_reply("unrelated", "ingest-write")
        .unwrap();
    misattributed
        .effects
        .read_back("unrelated", EffectState::Applied)
        .unwrap();
    assert_eq!(
        misattributed.validate(&p),
        Err(FaultReportError::LostByNonLosingEpisode {
            identity: "unrelated".to_string(),
            episode: "ingest-write".to_string(),
        }),
        "a CAS write fault loses no reply; the lost reply is still lost-ack's"
    );
    let mut ghost_loser = ok.clone();
    ghost_loser
        .effects
        .effects
        .get_mut("ack:1")
        .unwrap()
        .lost_by = ["ghost".to_string()].into_iter().collect();
    assert_eq!(
        ghost_loser.validate(&p),
        Err(FaultReportError::UnknownEpisode {
            episode: "ghost".to_string()
        })
    );
    let mut seen_pending = EffectLedger::default();
    seen_pending.attempt("x");
    seen_pending.lose_reply("x", "lost-ack").unwrap();
    seen_pending.observe("x").unwrap();
    assert_eq!(
        seen_pending.effects["x"].outcome,
        EffectOutcome::Applied,
        "an observed lost reply is known applied; the observation resolves it"
    );
    seen_pending.validate().unwrap();
    seen_pending.effects.get_mut("x").unwrap().outcome = EffectOutcome::Unknown;
    seen_pending.effects.get_mut("x").unwrap().expected = Expected::OneOf {
        states: [EffectState::Applied, EffectState::NotApplied]
            .into_iter()
            .collect(),
    };
    assert_eq!(
        seen_pending.validate(),
        Err(EffectRefused::ObservedWithoutReadBack {
            identity: "x".to_string()
        }),
        "a parsed entry observed yet still unknown claims an ambiguity the observation removed"
    );
    let mut no_pid = barrier("kill");
    no_pid.pid = 0;
    assert_eq!(
        no_pid.validate(),
        Err(BarrierRefused::NoPid {
            episode: "kill".to_string()
        })
    );
    let mut two_children = ok.clone();
    two_children.barriers.push(BarrierReceipt {
        pid: 4243,
        ..barrier("kill")
    });
    assert_eq!(
        two_children.validate(&p),
        Err(FaultReportError::DuplicateBarrier {
            episode: "kill".to_string(),
            cut: "acknowledged".to_string(),
        }),
        "one kill, one child, one barrier"
    );
    let mut invented_cut = ok.clone();
    invented_cut.coverage.declared.remove("acknowledged");
    invented_cut.coverage.receipted.remove("acknowledged");
    assert_eq!(
        invented_cut.validate(&p),
        Err(FaultReportError::Coverage(CoverageRefused::UndeclaredCut {
            cut: "acknowledged".to_string()
        })),
        "a kill's cut is one the campaign declared, so coverage must receipt it"
    );
    assert!(
        FaultAction::ClaimMaterialization {
            fault: MaterializationFaultKind::LoseAcknowledgementReply
        }
        .loses_reply()
    );
    assert!(
        !FaultAction::ClaimMaterialization {
            fault: MaterializationFaultKind::FailAcknowledgement
        }
        .loses_reply(),
        "the materializer never calls the kernel for a failed acknowledgement; nothing was lost"
    );
    assert!(
        !FaultAction::EmbeddingPublication {
            fault: PublicationFaultKind::LoseLocalCommit
        }
        .loses_reply(),
        "a rolled-back commit whose reply says so is known, not lost"
    );
    assert!(
        !kill().loses_reply(),
        "a kill's cut fixes what committed before it"
    );
    for (action, heal, family, loses) in [
        (
            FaultAction::EmbeddingDispatch {
                fault: DispatchFaultKind::LoseChargeReply,
            },
            Heal::Consumed,
            StoreFamily::SearchProjection,
            true,
        ),
        (
            FaultAction::EmbeddingDispatch {
                fault: DispatchFaultKind::RefuseChargeStatement,
            },
            Heal::Consumed,
            StoreFamily::SearchProjection,
            false,
        ),
        (
            FaultAction::ArtifactGc {
                fault: ArtifactGcFaultKind::Unlink,
            },
            Heal::Reopen,
            StoreFamily::Kernel,
            false,
        ),
        (
            FaultAction::ArtifactGc {
                fault: ArtifactGcFaultKind::AfterUnlink,
            },
            Heal::Consumed,
            StoreFamily::Kernel,
            true,
        ),
        (
            FaultAction::KernelRestore {
                fault: RestoreFaultKind::AfterDisplace,
            },
            Heal::Consumed,
            StoreFamily::Kernel,
            false,
        ),
        (
            FaultAction::KernelRestore {
                fault: RestoreFaultKind::RecoveryFailure,
            },
            Heal::Reopen,
            StoreFamily::Kernel,
            false,
        ),
        (
            FaultAction::ProjectionBatch {
                fault: BatchFaultKind::AfterRows,
            },
            Heal::Consumed,
            StoreFamily::SearchProjection,
            false,
        ),
    ] {
        assert_eq!(action.heal(), heal, "{action:?}");
        assert_eq!(action.family(), Some(family), "{action:?}");
        assert_eq!(action.loses_reply(), loses, "{action:?}");
        let value = serde_json::to_value(&action).unwrap();
        assert_eq!(
            serde_json::from_value::<FaultAction>(value).unwrap(),
            action
        );
    }
    assert_eq!(
        serde_json::to_value(FaultAction::ArtifactGc {
            fault: ArtifactGcFaultKind::FenceRaisedBeforeUnlink
        })
        .unwrap(),
        serde_json::json!({"kind": "artifact_gc", "fault": "fence_raised_before_unlink"})
    );
    let bare = BarrierReceipt {
        line: "acknowledged".to_string(),
        ..barrier("kill")
    };
    assert!(
        matches!(
            bare.validate(),
            Err(BarrierRefused::LineDoesNotNameCut { .. })
        ),
        "a bare cut is not the `<prefix> <cut>` line a child prints"
    );
    let mut all_acked = EffectLedger::default();
    all_acked.attempt("x");
    all_acked.acknowledge("x").unwrap();
    all_acked.lose_reply("x", "lost-ack").unwrap();
    all_acked.read_back("x", EffectState::Applied).unwrap();
    assert_eq!(
        all_acked.validate(),
        Err(EffectRefused::LostReplyAcknowledged {
            identity: "x".to_string()
        }),
        "every attempt acknowledged means no reply was lost"
    );

    assert!(
        !FaultAction::EmbeddingDispatch {
            fault: DispatchFaultKind::RefuseLedgerRead
        }
        .loses_reply(),
        "a refused ledger read blocks the read-back of a reply another fault lost"
    );
    let mut undeclared_fault = ok.clone();
    undeclared_fault.coverage.declared.remove("ingest-write");
    undeclared_fault.coverage.receipted.remove("ingest-write");
    assert_eq!(
        undeclared_fault.validate(&p),
        Err(FaultReportError::Coverage(CoverageRefused::UndeclaredCut {
            cut: "ingest-write".to_string()
        })),
        "every episode's firing point is a cut the report cannot drop"
    );

    let mut nameless = EffectLedger::default();
    nameless.attempt(" ");
    assert_eq!(
        nameless.validate(),
        Err(EffectRefused::EmptyIdentity),
        "a key that names no operation is no identity to bound"
    );

    let mut other_profile = ok.clone();
    other_profile.profile_digest = "ef".repeat(32);
    assert_eq!(
        other_profile.validate(&p),
        Err(FaultReportError::ProfileDigestMismatch),
        "the report names the profile whose bounds and limits gate it"
    );
    let mut retried = EffectLedger::default();
    retried.attempt("again");
    retried.lose_reply("again", "lost-ack").unwrap();
    retried.read_back("again", EffectState::NotApplied).unwrap();
    retried.attempt("again");
    retried.acknowledge("again").unwrap();
    let e = &retried.effects["again"];
    assert_eq!((e.attempted, e.observed, e.acknowledged), (2, 1, 1));
    assert_eq!(e.outcome, EffectOutcome::Applied);
    retried
        .validate()
        .expect("a retry that lands after a not-applied read-back is applied");
    let mut unseen = EffectLedger::default();
    unseen.attempt("ghost");
    unseen.lose_reply("ghost", "lost-ack").unwrap();
    unseen.read_back("ghost", EffectState::Applied).unwrap();
    unseen.effects.get_mut("ghost").unwrap().observed = 0;
    assert_eq!(
        unseen.validate(),
        Err(EffectRefused::OutcomeNotDerived {
            identity: "ghost".to_string()
        }),
        "an applied read-back is an observation; none recorded means none found"
    );

    let backup = FaultAction::BackupBeforeRename;
    assert_eq!(backup.heal(), Heal::Consumed);
    assert_eq!(backup.family(), Some(StoreFamily::Kernel));
    assert!(!backup.loses_reply());
    assert_eq!(
        serde_json::to_value(&backup).unwrap(),
        serde_json::json!({"kind": "backup_before_rename"})
    );
    assert_eq!(
        FaultAction::ArtifactGc {
            fault: ArtifactGcFaultKind::FenceRaisedBeforeUnlink
        }
        .heal(),
        Heal::Reopen,
        "a raised writer fence leaves the lease stale until reopen"
    );
    let mut merely_attempted = EffectLedger::default();
    merely_attempted.attempt("tried");
    assert_eq!(
        merely_attempted.validate(),
        Err(EffectRefused::OutcomeNotDerived {
            identity: "tried".to_string()
        }),
        "an attempt alone establishes nothing; an observation or acknowledgement does"
    );
    merely_attempted.observe("tried").unwrap();
    merely_attempted.validate().unwrap();
    let mut no_checkpoints = ok.clone();
    no_checkpoints.cuts.clear();
    assert_eq!(
        no_checkpoints.validate(&p),
        Err(FaultReportError::MissingCut {
            cut: Cut::AfterAtomicTransition
        }),
        "every oracle checkpoint is receipted, reached or not"
    );
    let mut twice_lost = EffectLedger::default();
    twice_lost.attempt("x");
    twice_lost.lose_reply("x", "first").unwrap();
    twice_lost.read_back("x", EffectState::NotApplied).unwrap();
    twice_lost.attempt("x");
    twice_lost.lose_reply("x", "second").unwrap();
    assert_eq!(
        twice_lost.effects["x"].lost_by,
        ["first".to_string(), "second".to_string()]
            .into_iter()
            .collect(),
        "a retried identity keeps every episode that lost one of its replies"
    );
    let mut overdriven = liveness();
    overdriven
        .lanes
        .get_mut(&Lane::CatchUpEpisodes)
        .unwrap()
        .steps = 65;
    assert!(
        matches!(
            overdriven.verdict(&b),
            Err(LivenessRefused::LivenessUnmet {
                lane: Lane::CatchUpEpisodes,
                progress_at_bound: 65,
                ..
            })
        ),
        "a lane is driven to its bound, not past it"
    );
    let mut uncounted = ok.clone();
    uncounted.envelope.peaks.processes = 0;
    assert_eq!(
        uncounted.validate(&p),
        Err(FaultReportError::KilledChildNotCounted),
        "the killed child was a process the envelope must have seen"
    );

    let mut double_loss = EffectLedger::default();
    double_loss.attempt("once");
    double_loss.lose_reply("once", "first").unwrap();
    double_loss.lose_reply("once", "second").unwrap();
    assert_eq!(
        double_loss.validate(),
        Err(EffectRefused::LostReplyAcknowledged {
            identity: "once".to_string()
        }),
        "one unacknowledged attempt loses one reply, not two"
    );
    let mut reopened = EffectLedger::default();
    reopened.attempt("re");
    reopened.lose_reply("re", "lost-ack").unwrap();
    reopened.read_back("re", EffectState::NotApplied).unwrap();
    reopened.attempt("re");
    let e = &reopened.effects["re"];
    assert_eq!(e.outcome, EffectOutcome::Unknown);
    assert!(
        !e.read_back,
        "the old read-back spoke for the attempt before this one"
    );
    reopened.validate().expect(
        "a retry is unresolved until something resolves it, and unresolved is a valid state",
    );
    assert_eq!(reopened.unknown(), ["re".to_string()].into_iter().collect());
    reopened.acknowledge("re").unwrap();
    assert_eq!(reopened.effects["re"].outcome, EffectOutcome::Applied);
    reopened.validate().unwrap();

    let mut lost_after_ack = EffectLedger::default();
    lost_after_ack.attempt("done");
    lost_after_ack.acknowledge("done").unwrap();
    lost_after_ack.attempt("done");
    lost_after_ack.lose_reply("done", "lost-ack").unwrap();
    let e = &lost_after_ack.effects["done"];
    assert_eq!(e.outcome, EffectOutcome::Applied);
    assert_eq!(e.lost_by, ["lost-ack".to_string()].into_iter().collect());
    lost_after_ack.validate().expect(
        "an acknowledged identity stays applied; a later lost retry is recorded, not doubted",
    );
    let mut unapproved = run_profile();
    unapproved.approval = None;
    assert!(
        matches!(
            unapproved.fault_profile(),
            Err(eval_core::ProfileError::NotApproved { .. })
        ),
        "the only constructor of a FaultProfile refuses an unapproved profile"
    );

    for (action, family, loses) in [
        (
            FaultAction::KernelCommitFailAfterEvents,
            StoreFamily::Kernel,
            false,
        ),
        (
            FaultAction::MessageCleanupLoseWriteReply,
            StoreFamily::SearchProjection,
            true,
        ),
        (
            FaultAction::IdentitySweepLoseReclaimReply,
            StoreFamily::SearchProjection,
            true,
        ),
    ] {
        assert_eq!(action.heal(), Heal::Consumed, "{action:?}");
        assert_eq!(action.family(), Some(family), "{action:?}");
        assert_eq!(action.loses_reply(), loses, "{action:?}");
        let value = serde_json::to_value(&action).unwrap();
        assert_eq!(
            serde_json::from_value::<FaultAction>(value).unwrap(),
            action
        );
    }
    let mut read_back_yet_unknown = EffectLedger::default();
    read_back_yet_unknown.attempt("rb");
    read_back_yet_unknown.lose_reply("rb", "lost-ack").unwrap();
    read_back_yet_unknown
        .effects
        .get_mut("rb")
        .unwrap()
        .read_back = true;
    assert_eq!(
        read_back_yet_unknown.validate(),
        Err(EffectRefused::OutcomeNotDerived {
            identity: "rb".to_string()
        }),
        "a read-back names one state; unknown after one is no read-back"
    );

    for (text, evidences) in [
        ("DeletionUnpropagated { commit_seq: 7 }", true),
        ("DeletionUnpropagated", true),
        ("DeletionUnpropagated(7)", true),
        ("NotDeletionUnpropagated", false),
        ("a message mentioning DeletionUnpropagated", false),
        ("DeletionUnpropagatedX", false),
        ("", false),
    ] {
        assert_eq!(
            ExpectedRefusal::R11DeletionBearingCatchUp.evidences(text),
            evidences,
            "{text:?}"
        );
    }
    let mut word_inside = ok.clone();
    word_inside.expected_refusals[0].production_error = "NotDeletionUnpropagated".to_string();
    assert_eq!(
        word_inside.validate(&p),
        Err(FaultReportError::RefusalNotEvidenced {
            episode: "r11".to_string(),
            refusal: ExpectedRefusal::R11DeletionBearingCatchUp,
        }),
        "the variant is evidenced by production's own text, not by a word containing it"
    );
    let mut claimed_twice = ok.clone();
    claimed_twice.effects.attempt("ack:2");
    claimed_twice
        .effects
        .lose_reply("ack:2", "lost-ack")
        .unwrap();
    claimed_twice
        .effects
        .read_back("ack:2", EffectState::Applied)
        .unwrap();
    assert_eq!(
        claimed_twice.validate(&p),
        Err(FaultReportError::LostReplyClaimedTwice {
            episode: "lost-ack".to_string()
        }),
        "one episode fires once and loses one reply"
    );
}

#[test]
fn fault_markers_each_name_a_test_here() {
    let mine: Vec<_> = eval_core::MARKERS
        .iter()
        .filter(|m| m.test.starts_with(SUITE))
        .collect();
    assert_eq!(mine.len(), 4);
    let mut coverage = Coverage::default();
    for marker in &mine {
        coverage.record(marker.name).unwrap();
    }
    coverage.complete(SUITE).unwrap();
}

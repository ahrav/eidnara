use std::collections::{BTreeMap, BTreeSet};

use eval_core::{
    APPLICATION_CRASH, ArtifactDeletionFaultKind, ArtifactIngestFaultKind, BarrierReceipt,
    BarrierRefused, ClaimBoundary, Coverage, CoverageRefused, Cut, CutCoverage, CutOutcome,
    EffectLedger, EffectOutcome, EffectRefused, EffectState, Envelope, EpisodeRefused, Expected,
    ExpectedRefusal, FAULT_REPORT_SCHEMA, FaultAction, FaultEpisode, FaultReport, FaultReportError,
    FaultScope, Heal, HealthyCore, KillLabel, Lane, LaneProgress, LivenessBounds, LivenessRefused,
    LivenessReport, MaterializationFaultKind, PublicationFaultKind, RecordedRefusal,
    ResourceLimits, SIGKILL, SearchEpisodeFault, StoreFamily, TEST_BINARY_CHILD, cut_receipts,
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
    effects.lose_reply("ack:1").unwrap();
    effects.read_back("ack:1", EffectState::Applied).unwrap();
    FaultReport {
        schema: FAULT_REPORT_SCHEMA.to_string(),
        eval_run_id: "ab".repeat(32),
        profile_digest: "cd".repeat(32),
        claim_boundary: ClaimBoundary::pinned(),
        episodes: vec![
            episode("lost-ack", lost_ack()),
            episode("kill", kill()),
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
            &[Cut::AtQuiescence, Cut::AfterRecovery, Cut::EndOfRun],
            &[
                (Cut::AtQuiescence, 1),
                (Cut::AfterRecovery, 2),
                (Cut::EndOfRun, 1),
            ]
            .into_iter()
            .collect(),
        ),
        coverage: coverage(&["lost-ack", "kill"]),
        effects,
        expected_refusals: vec![RecordedRefusal {
            episode: "lost-ack".to_string(),
            refusal: ExpectedRefusal::R11DeletionBearingCatchUp,
            production_error: "DeletionUnpropagated { commit_seq: 7 }".to_string(),
        }],
        safety_checks_while_armed: 5,
        liveness: Some(liveness()),
        markers: BTreeSet::new(),
        envelope: Envelope::new(limits()),
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
    ledger.lose_reply("commit:6").unwrap();
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
    acked_then_lost.lose_reply("commit:5").unwrap();
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
    ledger.lose_reply("ack:2").unwrap();
    ledger.effects.get_mut("ack:2").unwrap().outcome = EffectOutcome::Applied;
    assert_eq!(
        ledger.validate(),
        Err(EffectRefused::PrematureSuccess {
            identity: "ack:2".to_string()
        })
    );
    let mut collapsed = EffectLedger::default();
    collapsed.attempt("ack:3");
    collapsed.lose_reply("ack:3").unwrap();
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
    let value = report.serialize(&bounds()).unwrap();
    assert_eq!(parse_fault_report(&value, &bounds()).unwrap(), report);
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
        wrong_schema.validate(&bounds()),
        Err(FaultReportError::SchemaMismatch { .. })
    ));
    let mut no_barrier = report.clone();
    no_barrier.barriers.clear();
    assert_eq!(
        no_barrier.validate(&bounds()),
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
        wrong_cut.validate(&bounds()),
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
        ghost.validate(&bounds()),
        Err(FaultReportError::UnknownEpisode {
            episode: "ghost".to_string()
        }),
        "an armed outside-core fault the campaign never ran is not evidence"
    );
    let mut unreceipted = report.clone();
    unreceipted.coverage.declare("unlink");
    assert!(matches!(
        unreceipted.validate(&bounds()),
        Err(FaultReportError::Coverage(
            CoverageRefused::IncompleteCoverage { .. }
        ))
    ));
    let mut unsafe_run = report.clone();
    unsafe_run.safety_checks_while_armed = 0;
    assert_eq!(
        unsafe_run.validate(&bounds()),
        Err(FaultReportError::SafetyNeverChecked)
    );
    let mut premature = report.clone();
    premature.effects.attempt("ack:9");
    premature.effects.lose_reply("ack:9").unwrap();
    premature.effects.effects.get_mut("ack:9").unwrap().outcome = EffectOutcome::Applied;
    assert!(matches!(
        premature.validate(&bounds()),
        Err(FaultReportError::Effect(
            EffectRefused::PrematureSuccess { .. }
        ))
    ));
    let mut extra = value.clone();
    extra["surprise"] = serde_json::json!(1);
    assert!(matches!(
        parse_fault_report(&extra, &bounds()),
        Err(FaultReportError::Shape(_))
    ));
    let mut liveness_less = report;
    liveness_less.liveness = None;
    liveness_less.validate(&bounds()).unwrap();
    let _ = (
        PublicationFaultKind::LoseLocalCommit,
        BTreeMap::<String, u64>::new(),
    );
}

#[test]
fn a_parsed_report_cannot_claim_what_no_run_recorded() {
    let ok = report();
    let b = bounds();

    let mut boundary = ok.clone();
    boundary.claim_boundary.exclusions.clear();
    assert_eq!(
        boundary.validate(&b),
        Err(FaultReportError::ClaimBoundaryMismatch)
    );
    let mut over = ok.clone();
    over.envelope.peaks.processes = limits().processes + 1;
    assert!(matches!(
        over.validate(&b),
        Err(FaultReportError::EnvelopeExceeded(_))
    ));
    let mut no_fault = ok.clone();
    no_fault.episodes.clear();
    no_fault.barriers.clear();
    no_fault.coverage = CutCoverage::default();
    no_fault.liveness = None;
    assert_eq!(
        no_fault.validate(&b),
        Err(FaultReportError::NoEpisode),
        "nothing was armed, so no safety check ran while a fault was"
    );
    let mut invented = ok.clone();
    invented.markers.insert("flt_never_registered".to_string());
    assert_eq!(
        invented.validate(&b),
        Err(FaultReportError::UnregisteredMarker {
            marker: "flt_never_registered".to_string()
        })
    );
    let mut in_core = ok.clone();
    in_core.episodes[2].action = FaultAction::ExternalLockHolder;
    in_core.episodes[2].heal = Heal::Released;
    in_core.episodes[2].scope.store = StoreFamily::SearchProjection;
    assert_eq!(
        in_core.validate(&b),
        Err(FaultReportError::CoreFamilyFaulted {
            episode: "ingest-write".to_string(),
            store: StoreFamily::SearchProjection,
        }),
        "a fault scoped to a healthy-core family is not outside the core"
    );
    let mut consumed = ok.clone();
    consumed.episodes[2].action = FaultAction::ArtifactIngest {
        fault: ArtifactIngestFaultKind::ReservationCommit,
    };
    consumed.episodes[2].heal = Heal::Consumed;
    assert_eq!(
        consumed.validate(&b),
        Err(FaultReportError::ConsumedFaultArmed {
            episode: "ingest-write".to_string()
        }),
        "a one-shot fault is consumed or never fired; neither is armed at the bound"
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
    effect.reply_lost = false;
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
    contradicted.effects.get_mut("ack:1").unwrap().outcome = EffectOutcome::NotApplied;
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
    overcounted.lose_reply("dup").unwrap();
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
        no_run.validate(&b),
        Err(FaultReportError::MalformedDigest {
            field: "eval_run_id"
        })
    );
    let mut no_profile = ok.clone();
    no_profile.profile_digest = "ZZ".repeat(32);
    assert_eq!(
        no_profile.validate(&b),
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
    let mut mislabelled = ok.episodes[2].clone();
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
        ghost_refusal.validate(&b),
        Err(FaultReportError::UnknownEpisode {
            episode: "ghost".to_string()
        })
    );
    let mut unevidenced = ok.clone();
    unevidenced.expected_refusals[0].production_error = String::new();
    assert_eq!(
        unevidenced.validate(&b),
        Err(FaultReportError::RefusalNotEvidenced {
            episode: "lost-ack".to_string(),
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
        stalled_ghost.validate(&b),
        Err(FaultReportError::UnknownEpisode {
            episode: "ghost".to_string()
        }),
        "a permanent stall is a recorded refusal too"
    );

    let mut orphan_barrier = ok.clone();
    orphan_barrier.barriers.push(barrier("ingest-write"));
    assert_eq!(
        orphan_barrier.validate(&b),
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
        other_cut.validate(&b),
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
        twice.validate(&b),
        Err(FaultReportError::DuplicateCut {
            cut: Cut::AtQuiescence
        }),
        "two outcomes for one checkpoint is no outcome"
    );
    let mut untried = ok.effects.clone();
    let effect = untried.effects.get_mut("ack:1").unwrap();
    effect.attempted = 0;
    effect.observed = 0;
    effect.reply_lost = false;
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
            unsafe_int.serialize(&b),
            Err(FaultReportError::NotCanonical(_))
        ),
        "a report the result digest refuses is not serialized as valid"
    );
    let mut unsafe_value = ok.serialize(&b).unwrap();
    unsafe_value["safety_checks_while_armed"] = serde_json::json!(9_007_199_254_740_992u64);
    assert!(matches!(
        parse_fault_report(&unsafe_value, &b),
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

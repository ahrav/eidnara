use std::collections::{BTreeMap, BTreeSet};

use eval_core::{
    APPLICATION_CRASH, ArtifactDeletionFaultKind, ArtifactIngestFaultKind, BarrierReceipt,
    BarrierRefused, ClaimBoundary, Coverage, CoverageRefused, Cut, CutCoverage, CutOutcome,
    EffectLedger, EffectOutcome, EffectRefused, EffectState, Envelope, EpisodeRefused, Expected,
    ExpectedRefusal, FAULT_REPORT_SCHEMA, FaultAction, FaultEpisode, FaultReport, FaultReportError,
    FaultScope, Heal, HealthyCore, KillLabel, Lane, LaneProgress, LivenessBounds, LivenessRefused,
    LivenessReport, PublicationFaultKind, RecordedRefusal, ResourceLimits, SearchEpisodeFault,
    StoreFamily, TEST_BINARY_CHILD, cut_receipts, parse_fault_report, validate_episodes,
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
        signal: 9,
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
            families: [StoreFamily::Kernel, StoreFamily::SearchProjection]
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
            episode("r11", r11()),
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
        coverage: coverage(&["lost-ack", "kill", "r11"]),
        effects,
        expected_refusals: vec![RecordedRefusal {
            episode: "r11".to_string(),
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
        assert_eq!(
            FaultAction::ArtifactIngest { fault }.heal(),
            Heal::Consumed,
            "{fault:?} fails its transaction and leaves ingestion open"
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
            episode: "kill".to_string()
        })
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
    let mut borrowed = report.clone();
    borrowed.episodes[2].action = FaultAction::ArtifactDeletion {
        fault: ArtifactDeletionFaultKind::AfterCommit,
    };
    borrowed.episodes[2].heal = Heal::Consumed;
    assert_eq!(
        borrowed.validate(&bounds()),
        Err(FaultReportError::RefusalNotDeclared {
            episode: "r11".to_string()
        }),
        "a refusal recorded against an injected fault's label claims a fault that never ran"
    );
    let mut mislabelled = report.clone();
    mislabelled.expected_refusals[0].refusal = ExpectedRefusal::R24ReceiptQuotaExhausted;
    assert_eq!(
        mislabelled.validate(&bounds()),
        Err(FaultReportError::RefusalNotDeclared {
            episode: "r11".to_string()
        })
    );
    let mut undeclared = report.clone();
    undeclared.expected_refusals[0].episode = "r24".to_string();
    assert_eq!(
        undeclared.validate(&bounds()),
        Err(FaultReportError::RefusalNotDeclared {
            episode: "r24".to_string()
        })
    );
    let mut unrecorded = report.clone();
    unrecorded.expected_refusals.clear();
    assert_eq!(
        unrecorded.validate(&bounds()),
        Err(FaultReportError::RefusalNotRecorded {
            episode: "r11".to_string()
        }),
        "an expected-refusal episode with no recorded production error claims a refusal the run never observed"
    );
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

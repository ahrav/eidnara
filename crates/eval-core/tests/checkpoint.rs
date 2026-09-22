use std::collections::{BTreeMap, BTreeSet};

use eval_core::{
    AGING_REPORT_SCHEMA, AgingReport, AgingReportError, Checkpoint, CheckpointRefused,
    ClaimBoundary, ConstructionKind, Death, Descriptor, Divergence, Envelope, GenerationState,
    GuardComparison, HistoricalRows, LiveRows, PrefixRefused, ProjectionRows, QuiescenceReceipt,
    Reopened, ResourceLimits, RestoreRefused, Segment, StateSnapshot, StoreFamily, StoreIntegrity,
    StoreQuiescence, TombstoneReason, Unenumerated, WalCheckpoint, WindowDeaths, WorkCounter,
    historical_diff, live_digest, parse_aging_report,
};
use serde_json::json;

const INCARNATION: &str = "3ce762fc009cc0bfb468992e74d67d53";

fn truncated() -> WalCheckpoint {
    WalCheckpoint {
        busy: 0,
        wal_frames: 12,
        checkpointed_frames: 12,
    }
}

fn quiet(family: StoreFamily) -> StoreQuiescence {
    StoreQuiescence {
        pending: family.counters().iter().map(|c| (*c, 0)).collect(),
        wal: truncated(),
        wal_sidecar_bytes: 0,
        handles_closed: true,
    }
}

fn receipt() -> QuiescenceReceipt {
    QuiescenceReceipt {
        step: 7,
        stores: StoreFamily::ALL
            .into_iter()
            .map(|family| (family, quiet(family)))
            .collect(),
    }
}

fn files() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("kernel/kernel.sqlite".to_string(), "a".repeat(64)),
        ("memory.sqlite".to_string(), "b".repeat(64)),
        ("search/search.sqlite".to_string(), "c".repeat(64)),
    ])
}

fn checkpoint() -> Checkpoint {
    Checkpoint::new(receipt(), INCARNATION.to_string(), files()).unwrap()
}

#[test]
fn an_all_zero_receipt_over_every_family_permits_the_copy() {
    let checkpoint = checkpoint();
    assert_eq!(checkpoint.receipt.step, 7);
    let digest = checkpoint.digest().unwrap();
    assert_eq!(digest, checkpoint.digest().unwrap());
    let mut later = receipt();
    later.step = 8;
    let other = Checkpoint::new(later, INCARNATION.to_string(), files()).unwrap();
    assert_ne!(other.digest().unwrap(), digest);
    assert_eq!(
        serde_json::to_value(&checkpoint.receipt.stores[&StoreFamily::Kernel].pending).unwrap(),
        json!({"outbox_unpublished": 0})
    );
}

#[test]
fn a_receipt_missing_a_family_refuses_rather_than_borrowing_the_kernels() {
    let mut receipt = receipt();
    receipt.stores.remove(&StoreFamily::Memory);
    assert_eq!(
        Checkpoint::admit(&receipt, INCARNATION),
        Err(CheckpointRefused::MissingStoreEvidence {
            family: StoreFamily::Memory,
        })
    );
}

#[test]
fn a_receipt_missing_a_counter_is_not_quiescence() {
    let mut receipt = receipt();
    receipt
        .stores
        .get_mut(&StoreFamily::SearchProjection)
        .unwrap()
        .pending
        .remove(&WorkCounter::EmbeddingOpen);
    assert_eq!(
        Checkpoint::admit(&receipt, INCARNATION),
        Err(CheckpointRefused::MissingCounter {
            family: StoreFamily::SearchProjection,
            counter: WorkCounter::EmbeddingOpen,
        })
    );
    let mut empty = receipt.clone();
    empty
        .stores
        .get_mut(&StoreFamily::Kernel)
        .unwrap()
        .pending
        .clear();
    assert_eq!(
        Checkpoint::admit(&empty, INCARNATION),
        Err(CheckpointRefused::MissingCounter {
            family: StoreFamily::Kernel,
            counter: WorkCounter::OutboxUnpublished,
        })
    );
}

#[test]
fn pending_work_refuses_by_family_and_counter() {
    let mut pending = receipt();
    pending
        .stores
        .get_mut(&StoreFamily::SearchProjection)
        .unwrap()
        .pending
        .insert(WorkCounter::CatchUpLag, 3);
    assert_eq!(
        Checkpoint::admit(&pending, INCARNATION),
        Err(CheckpointRefused::PendingWork {
            family: StoreFamily::SearchProjection,
            counter: WorkCounter::CatchUpLag,
            observed: 3,
        })
    );
}

/// A counter outside the family's declared set invalidates the receipt,
/// whatever its value.
#[test]
fn a_counter_the_family_does_not_declare_is_refused() {
    let mut stray = receipt();
    stray
        .stores
        .get_mut(&StoreFamily::Kernel)
        .unwrap()
        .pending
        .insert(WorkCounter::CatchUpLag, 5);
    assert_eq!(
        Checkpoint::admit(&stray, INCARNATION),
        Err(CheckpointRefused::UndeclaredCounter {
            family: StoreFamily::Kernel,
            counter: WorkCounter::CatchUpLag,
        })
    );
    let mut quiet_stray = receipt();
    quiet_stray
        .stores
        .get_mut(&StoreFamily::Memory)
        .unwrap()
        .pending
        .insert(WorkCounter::OutboxUnpublished, 0);
    assert_eq!(
        Checkpoint::admit(&quiet_stray, INCARNATION),
        Err(CheckpointRefused::UndeclaredCounter {
            family: StoreFamily::Memory,
            counter: WorkCounter::OutboxUnpublished,
        })
    );
}

#[test]
fn a_wal_that_is_busy_partial_or_absent_is_not_truncated() {
    let busy = WalCheckpoint {
        busy: 1,
        wal_frames: 12,
        checkpointed_frames: 4,
    };
    assert!(!busy.is_truncated());
    assert!(
        !WalCheckpoint {
            busy: 0,
            wal_frames: 12,
            checkpointed_frames: 11,
        }
        .is_truncated()
    );
    let not_wal = WalCheckpoint {
        busy: 0,
        wal_frames: -1,
        checkpointed_frames: -1,
    };
    assert!(!not_wal.is_truncated());
    assert!(
        WalCheckpoint {
            busy: 0,
            wal_frames: 0,
            checkpointed_frames: 0,
        }
        .is_truncated()
    );
    let mut held = receipt();
    held.stores.get_mut(&StoreFamily::Kernel).unwrap().wal = busy;
    assert_eq!(
        Checkpoint::admit(&held, INCARNATION),
        Err(CheckpointRefused::WalNotTruncated {
            family: StoreFamily::Kernel,
            wal: busy,
        })
    );
    let mut sidecar = receipt();
    sidecar
        .stores
        .get_mut(&StoreFamily::Memory)
        .unwrap()
        .wal_sidecar_bytes = 4096;
    assert_eq!(
        Checkpoint::admit(&sidecar, INCARNATION),
        Err(CheckpointRefused::WalSidecarPresent {
            family: StoreFamily::Memory,
            bytes: 4096,
        })
    );
}

#[test]
fn an_open_handle_a_malformed_incarnation_and_no_files_refuse() {
    let mut open = receipt();
    open.stores
        .get_mut(&StoreFamily::Memory)
        .unwrap()
        .handles_closed = false;
    assert_eq!(
        Checkpoint::admit(&open, INCARNATION),
        Err(CheckpointRefused::HandleOpen {
            family: StoreFamily::Memory,
        })
    );
    for malformed in ["ABC", &INCARNATION.to_uppercase(), &INCARNATION[..31]] {
        assert_eq!(
            Checkpoint::admit(&receipt(), malformed),
            Err(CheckpointRefused::MalformedIncarnation(
                malformed.to_string()
            ))
        );
    }
    assert_eq!(
        Checkpoint::new(receipt(), INCARNATION.to_string(), BTreeMap::new()),
        Err(CheckpointRefused::NoFiles)
    );
    for (path, digest) in [
        ("kernel/kernel.sqlite", String::new()),
        ("kernel/kernel.sqlite", "A".repeat(64)),
        ("kernel/kernel.sqlite", "a".repeat(63)),
        ("", "a".repeat(64)),
    ] {
        let mut malformed = files();
        malformed.insert(path.to_string(), digest);
        assert_eq!(
            Checkpoint::new(receipt(), INCARNATION.to_string(), malformed),
            Err(CheckpointRefused::MalformedFile {
                path: path.to_string(),
            })
        );
    }
}

fn intact() -> StoreIntegrity {
    StoreIntegrity {
        integrity_check: "ok".to_string(),
        foreign_key_violations: 0,
    }
}

fn reopened(incarnation: &str) -> Reopened {
    Reopened {
        incarnation_id: incarnation.to_string(),
        stores: StoreFamily::ALL
            .into_iter()
            .map(|family| (family, intact()))
            .collect(),
        files: files(),
    }
}

#[test]
fn a_reopened_copy_is_accepted_only_as_the_same_intact_store() {
    let checkpoint = checkpoint();
    checkpoint.accept(&reopened(INCARNATION)).unwrap();
    let foreign = "d19d2c6200ee10b9b3633f7e40d6c952";
    assert_eq!(
        checkpoint.accept(&reopened(foreign)),
        Err(RestoreRefused::ForeignIncarnation {
            expected: INCARNATION.to_string(),
            found: foreign.to_string(),
        })
    );
    let mut missing = reopened(INCARNATION);
    missing.stores.remove(&StoreFamily::SearchProjection);
    assert_eq!(
        checkpoint.accept(&missing),
        Err(RestoreRefused::MissingStore {
            family: StoreFamily::SearchProjection,
        })
    );
    let mut torn = reopened(INCARNATION);
    torn.stores
        .get_mut(&StoreFamily::Kernel)
        .unwrap()
        .integrity_check = "*** in database main *** Page 3: never used".to_string();
    assert!(matches!(
        checkpoint.accept(&torn),
        Err(RestoreRefused::IntegrityCheck {
            family: StoreFamily::Kernel,
            ..
        })
    ));
    let mut dangling = reopened(INCARNATION);
    dangling
        .stores
        .get_mut(&StoreFamily::Memory)
        .unwrap()
        .foreign_key_violations = 2;
    assert_eq!(
        checkpoint.accept(&dangling),
        Err(RestoreRefused::ForeignKeyViolations {
            family: StoreFamily::Memory,
            count: 2,
        })
    );
    let mut lost = reopened(INCARNATION);
    lost.files.remove("memory.sqlite");
    assert_eq!(
        checkpoint.accept(&lost),
        Err(RestoreRefused::FileMissing {
            path: "memory.sqlite".to_string(),
        })
    );
    let mut changed = reopened(INCARNATION);
    changed
        .files
        .insert("search/search.sqlite".to_string(), "d".repeat(64));
    assert_eq!(
        checkpoint.accept(&changed),
        Err(RestoreRefused::FileDiffers {
            path: "search/search.sqlite".to_string(),
        })
    );
}

fn projection(
    snapshot: i64,
    occurrences: &[(&str, i64)],
    tombstones: &[(&str, i64, TombstoneReason)],
) -> ProjectionRows {
    let dead: BTreeSet<&str> = tombstones.iter().map(|(id, _, _)| *id).collect();
    ProjectionRows {
        snapshot_commit_seq: snapshot,
        live: LiveRows {
            occurrences: occurrences
                .iter()
                .filter(|(id, _)| !dead.contains(id))
                .map(|(id, _)| (id.to_string(), format!("payload-{id}")))
                .collect(),
            lexical: occurrences
                .iter()
                .filter(|(id, _)| !dead.contains(id))
                .map(|(id, _)| id.to_string())
                .collect(),
            pending_embedding: BTreeSet::new(),
        },
        historical: HistoricalRows {
            occurrences: occurrences
                .iter()
                .map(|(id, created)| (id.to_string(), *created))
                .collect(),
            tombstones: tombstones
                .iter()
                .map(|(id, at, reason)| {
                    (
                        id.to_string(),
                        Death {
                            invalidated_commit_seq: *at,
                            reason: *reason,
                        },
                    )
                })
                .collect(),
            generation_state: GenerationState::Building,
        },
    }
}

fn death(at: i64, reason: TombstoneReason) -> Death {
    Death {
        invalidated_commit_seq: at,
        reason,
    }
}

#[test]
fn the_live_digest_sees_every_live_row() {
    let base = projection(1, &[("k1r1", 2), ("k2r1", 3)], &[]);
    let digest = live_digest(&base.live).unwrap();
    assert_eq!(digest, live_digest(&base.live.clone()).unwrap());
    let mut payload = base.live.clone();
    payload
        .occurrences
        .insert("k1r1".to_string(), "other".to_string());
    assert_ne!(live_digest(&payload).unwrap(), digest);
    let mut lexical = base.live.clone();
    lexical.lexical.remove("k2r1");
    assert_ne!(live_digest(&lexical).unwrap(), digest);
    let mut pending = base.live.clone();
    pending.pending_embedding.insert("k2r1".to_string());
    assert_ne!(live_digest(&pending).unwrap(), digest);
}

#[test]
fn the_enumerated_divergence_is_a_death_between_the_two_snapshots() {
    let earlier = projection(
        1,
        &[("k1r1", 2), ("k2r1", 3), ("k1r2", 5), ("k3r1", 9)],
        &[
            ("k1r1", 5, TombstoneReason::Superseded),
            ("k2r1", 6, TombstoneReason::Retired),
        ],
    );
    let later = projection(7, &[("k1r2", 5), ("k3r1", 9)], &[]);
    let comparison = GuardComparison::of(
        (&earlier, ConstructionKind::CatchUp),
        (&later, ConstructionKind::Bulk),
    )
    .unwrap();
    assert!(comparison.live_digests_equal);
    assert_eq!(comparison.earlier.snapshot_commit_seq, 1);
    assert_eq!(comparison.later.kind, ConstructionKind::Bulk);
    assert_eq!(
        comparison.divergences,
        vec![
            Divergence::TombstonedBeforeSnapshot {
                occurrence_id: "k1r1".to_string(),
                death: death(5, TombstoneReason::Superseded),
            },
            Divergence::TombstonedBeforeSnapshot {
                occurrence_id: "k2r1".to_string(),
                death: death(6, TombstoneReason::Retired),
            },
        ]
    );
    let mut verified = later.clone();
    verified.historical.generation_state = GenerationState::Verified;
    assert!(
        historical_diff(&earlier, &verified)
            .unwrap()
            .contains(&Divergence::GenerationState {
                earlier: GenerationState::Building,
                later: GenerationState::Verified,
            })
    );
    let mut both = (earlier.clone(), later.clone());
    for side in [&mut both.0, &mut both.1] {
        side.historical.occurrences.insert("k4r1".to_string(), 8);
        side.historical
            .tombstones
            .insert("k4r1".to_string(), death(9, TombstoneReason::Retired));
    }
    assert_eq!(
        historical_diff(&both.0, &both.1).unwrap(),
        historical_diff(&earlier, &later).unwrap()
    );
}

#[test]
fn the_death_window_is_open_at_the_earlier_snapshot_and_closed_at_the_later() {
    let dead = |at| {
        projection(
            1,
            &[("k1r1", 1), ("k2r1", 3)],
            &[("k1r1", at, TombstoneReason::Retired)],
        )
    };
    let later = projection(7, &[("k2r1", 3)], &[]);
    assert_eq!(
        historical_diff(&dead(7), &later).unwrap(),
        vec![Divergence::TombstonedBeforeSnapshot {
            occurrence_id: "k1r1".to_string(),
            death: death(7, TombstoneReason::Retired),
        }]
    );
    assert_eq!(
        historical_diff(&dead(1), &later),
        Err(Unenumerated::OccurrenceOnlyInEarlier {
            occurrence_id: "k1r1".to_string(),
        })
    );
    assert_eq!(
        historical_diff(&dead(8), &later),
        Err(Unenumerated::OccurrenceOnlyInEarlier {
            occurrence_id: "k1r1".to_string(),
        })
    );
}

#[test]
fn every_other_historical_difference_is_unenumerated() {
    let earlier = projection(
        1,
        &[("k1r1", 2), ("k2r1", 3)],
        &[("k1r1", 5, TombstoneReason::Superseded)],
    );
    let later = projection(7, &[("k2r1", 3)], &[]);
    assert_eq!(
        historical_diff(&later, &earlier),
        Err(Unenumerated::SnapshotOrder {
            earlier: 7,
            later: 1,
        })
    );
    let mut never_died = earlier.clone();
    never_died.historical.tombstones.clear();
    assert_eq!(
        historical_diff(&never_died, &later),
        Err(Unenumerated::OccurrenceOnlyInEarlier {
            occurrence_id: "k1r1".to_string(),
        })
    );
    let mut extra = later.clone();
    extra.historical.occurrences.insert("k9r1".to_string(), 8);
    assert_eq!(
        historical_diff(&earlier, &extra),
        Err(Unenumerated::OccurrenceOnlyInLater {
            occurrence_id: "k9r1".to_string(),
        })
    );
    let mut tombstoned_later = later.clone();
    tombstoned_later
        .historical
        .tombstones
        .insert("k2r1".to_string(), death(8, TombstoneReason::Retired));
    assert_eq!(
        historical_diff(&earlier, &tombstoned_later),
        Err(Unenumerated::TombstoneOnlyInLater {
            occurrence_id: "k2r1".to_string(),
        })
    );
    let mut tombstoned_earlier = earlier.clone();
    tombstoned_earlier
        .historical
        .tombstones
        .insert("k2r1".to_string(), death(8, TombstoneReason::Retired));
    assert_eq!(
        historical_diff(&tombstoned_earlier, &later),
        Err(Unenumerated::TombstoneOnlyInEarlier {
            occurrence_id: "k2r1".to_string(),
        })
    );
    let mut differs = tombstoned_later.clone();
    differs
        .historical
        .tombstones
        .get_mut("k2r1")
        .unwrap()
        .reason = TombstoneReason::Purged;
    assert_eq!(
        historical_diff(&tombstoned_earlier, &differs),
        Err(Unenumerated::TombstoneDiffers {
            occurrence_id: "k2r1".to_string(),
        })
    );
    let mut recreated = later.clone();
    recreated
        .historical
        .occurrences
        .insert("k2r1".to_string(), 4);
    assert_eq!(
        historical_diff(&earlier, &recreated),
        Err(Unenumerated::CreatedDiffers {
            occurrence_id: "k2r1".to_string(),
        })
    );
    let mut orphan_later = later.clone();
    orphan_later
        .historical
        .tombstones
        .insert("k1r1".to_string(), death(5, TombstoneReason::Superseded));
    assert_eq!(
        historical_diff(&earlier, &orphan_later),
        Err(Unenumerated::OrphanTombstone {
            occurrence_id: "k1r1".to_string(),
        })
    );
    let mut orphan_earlier = earlier.clone();
    orphan_earlier
        .historical
        .tombstones
        .insert("k7r1".to_string(), death(6, TombstoneReason::Retired));
    assert_eq!(
        historical_diff(&orphan_earlier, &later),
        Err(Unenumerated::OrphanTombstone {
            occurrence_id: "k7r1".to_string(),
        })
    );
}

#[test]
fn tombstone_reasons_and_generation_states_are_the_projections_closed_sets() {
    assert_eq!(
        serde_json::from_value::<TombstoneReason>(json!("evidence_invalidated")).unwrap(),
        TombstoneReason::EvidenceInvalidated
    );
    assert!(serde_json::from_value::<TombstoneReason>(json!("expired")).is_err());
    assert_eq!(
        serde_json::from_value::<GenerationState>(json!("selected")).unwrap(),
        GenerationState::Selected
    );
    assert!(serde_json::from_value::<GenerationState>(json!("pending")).is_err());
}

fn snapshot() -> StateSnapshot {
    StateSnapshot {
        commit_seq: 41,
        kernel: BTreeMap::from([(
            "srcdesc:k1".to_string(),
            Descriptor {
                source_revision: 1,
                created_commit_seq: 2,
                invalidated_commit_seq: Some(5),
                superseded_by: Some("srcdesc:k1b".to_string()),
            },
        )]),
        projection_live: LiveRows {
            occurrences: BTreeMap::from([("k1r2".to_string(), "e".repeat(64))]),
            lexical: BTreeSet::from(["k1r2".to_string()]),
            pending_embedding: BTreeSet::new(),
        },
        memory: BTreeMap::from([(
            1,
            Segment {
                start_message: 1,
                end_message: 1,
                content: "alpha for slot1 in world4".to_string(),
            },
        )]),
    }
}

#[test]
fn a_resumed_life_matches_the_full_replay_or_names_the_family_that_slipped() {
    let full = snapshot();
    StateSnapshot::compare(&full, &snapshot()).unwrap();
    assert_eq!(
        full.guard_digest().unwrap(),
        snapshot().guard_digest().unwrap()
    );
    assert_eq!(
        full.guard_digest().unwrap(),
        "7d2101fafa0a2df98de4274460d8e277966ebf0cd70b11c06238fab4422b45ba"
    );
    let mut ahead = snapshot();
    ahead.commit_seq += 1;
    assert_eq!(
        StateSnapshot::compare(&full, &ahead),
        Err(PrefixRefused::CommitSeqDiffers {
            full: 41,
            resumed: 42,
        })
    );
    let mut kernel = snapshot();
    kernel
        .kernel
        .get_mut("srcdesc:k1")
        .unwrap()
        .invalidated_commit_seq = None;
    assert_eq!(
        StateSnapshot::compare(&full, &kernel),
        Err(PrefixRefused::HistorySlipped {
            family: StoreFamily::Kernel,
        })
    );
    let mut memory = snapshot();
    memory.memory.get_mut(&1).unwrap().content.push('!');
    assert_eq!(
        StateSnapshot::compare(&full, &memory),
        Err(PrefixRefused::HistorySlipped {
            family: StoreFamily::Memory,
        })
    );
    let mut projection = snapshot();
    projection.projection_live.lexical.clear();
    assert_eq!(
        StateSnapshot::compare(&full, &projection),
        Err(PrefixRefused::HistorySlipped {
            family: StoreFamily::SearchProjection,
        })
    );
    for slipped in [&ahead, &kernel, &memory, &projection] {
        assert_ne!(
            slipped.guard_digest().unwrap(),
            full.guard_digest().unwrap()
        );
    }
}

#[test]
fn a_resumed_life_advances_the_tip_and_creates_nothing_before_the_checkpoint() {
    let reopened = snapshot();
    let mut resumed = snapshot();
    resumed.commit_seq = 50;
    resumed.kernel.insert(
        "srcdesc:k2".to_string(),
        Descriptor {
            source_revision: 1,
            created_commit_seq: 44,
            invalidated_commit_seq: None,
            superseded_by: None,
        },
    );
    StateSnapshot::advanced(&reopened, &resumed).unwrap();
    assert_eq!(
        StateSnapshot::advanced(&reopened, &reopened),
        Err(PrefixRefused::CommitSeqNotMonotonic {
            at_checkpoint: 41,
            at_end: 41,
        })
    );
    let mut rewritten = resumed.clone();
    rewritten
        .kernel
        .get_mut("srcdesc:k2")
        .unwrap()
        .created_commit_seq = 40;
    assert_eq!(
        StateSnapshot::advanced(&reopened, &rewritten),
        Err(PrefixRefused::HistoryRewritten {
            object_id: "srcdesc:k2".to_string(),
        })
    );
    let live = Descriptor {
        source_revision: 1,
        created_commit_seq: 3,
        invalidated_commit_seq: None,
        superseded_by: None,
    };
    let mut reopened = reopened;
    reopened
        .kernel
        .insert("srcdesc:k3".to_string(), live.clone());
    resumed.kernel.insert("srcdesc:k3".to_string(), live);
    StateSnapshot::advanced(&reopened, &resumed).unwrap();
    let mut died_later = resumed.clone();
    died_later
        .kernel
        .get_mut("srcdesc:k3")
        .unwrap()
        .invalidated_commit_seq = Some(45);
    StateSnapshot::advanced(&reopened, &died_later).unwrap();
    let mut backdated = resumed.clone();
    backdated
        .kernel
        .get_mut("srcdesc:k3")
        .unwrap()
        .invalidated_commit_seq = Some(41);
    assert_eq!(
        StateSnapshot::advanced(&reopened, &backdated),
        Err(PrefixRefused::HistoryRewritten {
            object_id: "srcdesc:k3".to_string(),
        })
    );
}

#[test]
fn window_deaths_count_descriptors_alive_at_the_snapshot_that_die_inside_the_window() {
    let descriptor = |created, invalidated, superseded: Option<&str>| Descriptor {
        source_revision: 1,
        created_commit_seq: created,
        invalidated_commit_seq: invalidated,
        superseded_by: superseded.map(str::to_string),
    };
    let descriptors = BTreeMap::from([
        ("a".to_string(), descriptor(1, Some(5), Some("b"))),
        ("b".to_string(), descriptor(5, None, None)),
        ("c".to_string(), descriptor(2, Some(6), None)),
        ("d".to_string(), descriptor(4, Some(7), None)),
        ("e".to_string(), descriptor(3, Some(9), None)),
        ("f".to_string(), descriptor(1, Some(3), None)),
    ]);
    assert_eq!(
        WindowDeaths::count(&descriptors, 3, 8),
        WindowDeaths {
            supersessions: 1,
            retirements: 1,
        }
    );
    assert_eq!(
        WindowDeaths::count(&descriptors, 0, 100),
        WindowDeaths::default()
    );
}

#[test]
fn the_aging_report_schema_is_pinned() {
    assert_eq!(AGING_REPORT_SCHEMA, "eval-suite-c-aging-report/v1");
}

fn aging_report() -> AgingReport {
    let checkpoint = checkpoint();
    let earlier = projection(
        1,
        &[("k1r1", 2), ("k2r1", 3)],
        &[("k1r1", 5, TombstoneReason::Superseded)],
    );
    let later = projection(7, &[("k2r1", 3)], &[]);
    let comparison = GuardComparison::of(
        (&earlier, ConstructionKind::CatchUp),
        (&later, ConstructionKind::Bulk),
    )
    .unwrap();
    let guard = snapshot().guard_digest().unwrap();
    AgingReport {
        schema: AGING_REPORT_SCHEMA.to_string(),
        eval_run_id: "1".repeat(64),
        profile_digest: "2".repeat(64),
        claim_boundary: ClaimBoundary::pinned(),
        steps: 12,
        checkpoint_step: checkpoint.receipt.step,
        checkpoint_digest: checkpoint.digest().unwrap(),
        receipt: checkpoint.receipt.clone(),
        full_guard_digest: guard.clone(),
        resumed_guard_digest: guard,
        commit_seq_at_checkpoint: 41,
        commit_seq_at_end: 50,
        against_resumed: comparison.clone(),
        against_bulk: comparison,
        window_deaths: WindowDeaths {
            supersessions: 1,
            retirements: 1,
        },
        markers: BTreeSet::from(["flt_quiescence_receipt_all_zero".to_string()]),
        envelope: Envelope::new(ResourceLimits {
            elapsed_ms: 60_000,
            store_bytes: 1 << 20,
            cassette_bytes: 1 << 20,
            artifact_bytes: 1 << 20,
            temp_roots: 4,
            retained_artifacts: 4,
            processes: 4,
        }),
    }
}

#[test]
fn an_aging_report_refuses_what_its_claims_and_checkpoint_forbid() {
    let report = aging_report();
    let value = report.serialize().unwrap();
    assert_eq!(parse_aging_report(&value).unwrap(), report);

    let mut unbounded = report.clone();
    unbounded.claim_boundary.exclusions.clear();
    assert_eq!(
        unbounded.validate(),
        Err(AgingReportError::ClaimBoundaryMismatch)
    );

    for field in [
        "eval_run_id",
        "profile_digest",
        "checkpoint_digest",
        "full_guard_digest",
        "resumed_guard_digest",
    ] {
        let mut malformed = report.clone();
        let slot = match field {
            "eval_run_id" => &mut malformed.eval_run_id,
            "profile_digest" => &mut malformed.profile_digest,
            "checkpoint_digest" => &mut malformed.checkpoint_digest,
            "full_guard_digest" => &mut malformed.full_guard_digest,
            _ => &mut malformed.resumed_guard_digest,
        };
        slot.pop();
        assert_eq!(
            malformed.validate(),
            Err(AgingReportError::MalformedDigest { field })
        );
    }

    let mut moved = report.clone();
    moved.checkpoint_step += 1;
    assert_eq!(
        moved.validate(),
        Err(AgingReportError::CheckpointStepMismatch {
            checkpoint_step: 8,
            receipt_step: 7,
        })
    );

    let mut still = report.clone();
    still.commit_seq_at_end = still.commit_seq_at_checkpoint;
    assert_eq!(
        still.validate(),
        Err(AgingReportError::CommitSeqNotMonotonic {
            at_checkpoint: 41,
            at_end: 41,
        })
    );
    let mut quiet = report.clone();
    quiet.window_deaths.retirements = 0;
    assert_eq!(
        quiet.validate(),
        Err(AgingReportError::WindowDeathsIncomplete {
            supersessions: 1,
            retirements: 0,
        })
    );

    let mut pending = report.clone();
    pending
        .receipt
        .stores
        .get_mut(&StoreFamily::Memory)
        .unwrap()
        .pending
        .insert(WorkCounter::CaptureJobsPending, 1);
    assert_eq!(
        pending.validate(),
        Err(AgingReportError::Receipt(CheckpointRefused::PendingWork {
            family: StoreFamily::Memory,
            counter: WorkCounter::CaptureJobsPending,
            observed: 1,
        }))
    );
    let mut borrowed = report.clone();
    borrowed.receipt.stores.remove(&StoreFamily::Memory);
    let borrowed_value = serde_json::to_value(&borrowed).unwrap();
    let missing = AgingReportError::Receipt(CheckpointRefused::MissingStoreEvidence {
        family: StoreFamily::Memory,
    });
    assert_eq!(borrowed.serialize(), Err(missing.clone()));
    assert_eq!(parse_aging_report(&borrowed_value), Err(missing));
}

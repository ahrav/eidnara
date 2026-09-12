//! Holds are compared with an independent ledger of which descriptors were live
//! at S, and every way a hold can stop protecting its bytes is exercised.

#![cfg(feature = "test-support")]

mod source_fixture;

use std::collections::BTreeSet;
use std::fs;
use std::num::{NonZeroU64, NonZeroUsize};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kernel::{
    ArtifactDeletionFault, ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest,
    ArtifactGcFault, KernelError, MAX_ACTIVE_SOURCE_HOLDS_PER_CONSUMER,
    MAX_SOURCE_HOLD_LIFETIME_MS, RemediationTarget, SourceHold, SourceHoldBinding,
    SourceHoldBounds, SourceHoldError, SourceHoldInvalidity,
};
use rusqlite::Connection;
use source_fixture::*;

fn assert_hold_matches_ledger(fixture: &Fixture, hold: &SourceHold, page_size: usize) {
    let (expected, references, bytes) = fixture.expected_at(hold.snapshot);
    assert_eq!(hold.references, references, "reference count at S");
    assert_eq!(hold.encoded_bytes, bytes, "encoded bytes at S");
    assert_eq!(
        fixture.held_all(hold, page_size),
        expected,
        "held inventory at S"
    );
    let mut expected_refs: Vec<String> = expected.iter().map(|d| d.evidence_id.clone()).collect();
    expected_refs.sort();
    expected_refs.dedup();
    assert_eq!(
        fixture.pin_refs(&hold.hold_id),
        expected_refs,
        "pinned evidence"
    );
}

/// What the concurrent capture run produced, joined after the writer stops.
struct ConcurrentRun {
    captured: Vec<SourceHold>,
    sweeps: usize,
    extended: Vec<(SourceHold, i64)>,
    held_after_delete: Vec<String>,
}

struct CheckpointAuthorization<'a> {
    fixture: &'a Fixture,
    through: i64,
    dropped: &'a AtomicBool,
}

impl Drop for CheckpointAuthorization<'_> {
    fn drop(&mut self) {
        assert_eq!(self.fixture.checkpoint(), self.through);
        self.dropped.store(true, Ordering::SeqCst);
    }
}

fn assert_hold_matches_ledger_inventory(fixture: &Fixture, hold: &SourceHold) {
    let (expected, _, _) = fixture.expected_at(hold.snapshot);
    assert_eq!(fixture.held_all(hold, 4), expected, "held inventory at S");
}

fn assert_extended_hold_matches_ledger(fixture: &Fixture, hold: &SourceHold, through: i64) {
    let (refs, bytes) = fixture.expected_refs_through(hold.snapshot, through);
    assert_eq!(
        hold.references,
        refs.len(),
        "reference count through {through}"
    );
    assert_eq!(hold.encoded_bytes, bytes, "encoded bytes through {through}");
    assert_eq!(
        fixture.pin_refs(&hold.hold_id),
        refs,
        "pinned evidence through {through}"
    );
}

#[test]
fn a_hold_equals_the_five_class_ledger_at_s_and_needs_a_registered_consumer() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    // Revise one, retire one, and cite one artifact from two classes so the
    // ledger has closed entries and a shared reference before S.
    let msg_a = fixture.publish("messages", "a", 2, "messages text a v2");
    let claim_b = fixture
        .live_entry("canonical_claims", "b")
        .object_id
        .clone();
    fixture.retire(&claim_b);
    let shared = fixture.retain("shared", "one artifact, two descriptors");
    fixture.publish_over(
        "promoted_memory",
        "shared",
        1,
        "one artifact, two descriptors",
        shared.clone(),
    );
    fixture.publish_over(
        "git_commits",
        "shared",
        1,
        "one artifact, two descriptors",
        shared,
    );
    let tip = fixture.store.tip().unwrap();

    let before = wall_ms();
    let hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), bounds(HOUR_MS))
        .unwrap();
    assert_eq!(hold.snapshot, tip, "S is the tip under the writer");
    assert_eq!(hold.binding, fixture.binding());
    assert!(hold.captured_at >= before && hold.captured_at <= wall_ms());
    assert_eq!(
        hold.expires_at - hold.captured_at,
        i64::try_from(HOUR_MS).unwrap()
    );
    for page_size in [1, 3, 64] {
        assert_hold_matches_ledger(&fixture, &hold, page_size);
    }
    let held = fixture.held_all(&hold, 64);
    assert_eq!(
        hold.references + 1,
        held.len(),
        "the shared artifact is one reference"
    );
    assert!(held.iter().any(|d| d.object_id == msg_a));
    assert!(
        !held.iter().any(|d| d.object_id == claim_b),
        "retired before S"
    );
    assert_eq!(
        held.iter()
            .map(|d| d.class.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        CLASSES.len()
    );
    assert_eq!(
        fixture
            .store
            .source_hold_status(&fixture.binding(), &hold.hold_id, hold.captured_at)
            .unwrap(),
        hold
    );
    let two_days = fixture
        .store
        .capture_source_hold(&fixture.binding(), bounds(2 * DAY_MS))
        .unwrap();
    assert_eq!(
        two_days.expires_at - two_days.captured_at,
        i64::try_from(2 * DAY_MS).unwrap()
    );

    // Repeated captures are bounded per owner; releasing one frees a slot.
    let mut extra = Vec::new();
    while extra.len() + 2 < MAX_ACTIVE_SOURCE_HOLDS_PER_CONSUMER {
        extra.push(
            fixture
                .store
                .capture_source_hold(&fixture.binding(), wide())
                .unwrap(),
        );
    }
    assert_eq!(
        fixture
            .store
            .capture_source_hold(&fixture.binding(), wide())
            .unwrap_err(),
        SourceHoldError::HoldLimitReached
    );
    // The cap is per consumer: a fresh policy version is not a fresh budget.
    let mut other_policy_at_cap = fixture.binding();
    other_policy_at_cap.source_policy_version = "source-policy.v2".to_string();
    assert_eq!(
        fixture
            .store
            .capture_source_hold(&other_policy_at_cap, wide())
            .unwrap_err(),
        SourceHoldError::HoldLimitReached
    );
    fixture
        .store
        .release_source_hold(&fixture.binding(), &two_days.hold_id, 1)
        .unwrap();
    let refilled = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    for stale in extra.iter().chain([&refilled]) {
        fixture
            .store
            .release_source_hold(&fixture.binding(), &stale.hold_id, 1)
            .unwrap();
    }

    // Registration precedes S; wrong consumer, incarnation, or policy fails.
    let mut wrong_consumer = fixture.binding();
    wrong_consumer.consumer_id = "nobody".to_string();
    assert_eq!(
        fixture
            .store
            .capture_source_hold(&wrong_consumer, wide())
            .unwrap_err(),
        SourceHoldError::UnknownConsumer
    );
    let mut wrong_epoch = fixture.binding();
    wrong_epoch.lease_epoch += 1;
    assert_eq!(
        fixture
            .store
            .capture_source_hold(&wrong_epoch, wide())
            .unwrap_err(),
        SourceHoldError::IncarnationMismatch
    );
    for policy in [
        "",
        "has space",
        &"p".repeat(129),
        "semi;colon",
        "unit\u{1f}sep",
    ] {
        let mut wrong_policy = fixture.binding();
        wrong_policy.source_policy_version = policy.to_string();
        assert_eq!(
            fixture
                .store
                .capture_source_hold(&wrong_policy, wide())
                .unwrap_err(),
            SourceHoldError::InvalidRequest,
            "{policy:?}"
        );
    }
    // A consumer id carrying the owner separator can never alias another
    // consumer's holds: it is refused before registration is consulted.
    fixture
        .store
        .commit(intent("separator-consumer"), |envelope| {
            envelope.register_outbox_consumer("search\u{1f}x", 1)?;
            Ok(String::new())
        })
        .unwrap();
    let mut separator = fixture.binding();
    separator.consumer_id = "search\u{1f}x".to_string();
    assert_eq!(
        fixture
            .store
            .capture_source_hold(&separator, wide())
            .unwrap_err(),
        SourceHoldError::InvalidRequest
    );
    // Using the hold under another binding fails before anything is read.
    let mut other_policy = fixture.binding();
    other_policy.source_policy_version = "source-policy.v2".to_string();
    assert_eq!(
        fixture
            .store
            .source_hold_status(&other_policy, &hold.hold_id, hold.captured_at)
            .unwrap_err(),
        SourceHoldError::BindingMismatch
    );
    assert_eq!(
        fixture
            .first_page(&wrong_consumer, &hold.hold_id, hold.captured_at)
            .unwrap_err(),
        SourceHoldError::BindingMismatch
    );
    assert_eq!(
        fixture.count(
            "SELECT COUNT(*) FROM capture_pins WHERE pin_kind='source_hold' AND released_at IS NULL"
        ),
        1,
        "only the first hold is still active"
    );
    assert_eq!(fixture.checkpoint(), 0, "capturing acknowledges nothing");
}

#[test]
fn writer_serialized_capture_leaves_no_gap_between_s_and_protection_under_concurrent_gc() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    // Two artifacts are logically deleted before the loop: reclaimable once
    // the grace period passes, unless a hold captured before the deletion
    // pins them. The sweep clock sits past the grace period the whole time.
    let long = bounds(MAX_SOURCE_HOLD_LIFETIME_MS);
    let pinned_before_delete = fixture
        .store
        .capture_source_hold(&fixture.binding(), long)
        .unwrap();
    let doomed: Vec<String> = ["messages", "raw_tool_spans"]
        .into_iter()
        .map(|class| fixture.live_entry(class, "b").evidence_id.clone())
        .collect();
    let deleted_at = wall_ms();
    for evidence_id in &doomed {
        fixture.delete_evidence(evidence_id, ArtifactDeletionKind::Delete, deleted_at);
    }
    let sweep_now = wall_ms() + i64::try_from(15 * DAY_MS).unwrap();
    let first_publish = fixture.store.tip().unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let store = Arc::clone(&fixture.store);
    let binding = fixture.binding();
    let run: ConcurrentRun = std::thread::scope(|scope| {
        let stop_reader = Arc::clone(&stop);
        let reader_store = Arc::clone(&store);
        let reader = scope.spawn(move || {
            let mut holds = Vec::new();
            while !stop_reader.load(Ordering::Relaxed) && holds.len() < 16 {
                holds.push(reader_store.capture_source_hold(&binding, long).unwrap());
            }
            holds
        });
        let stop_gc = Arc::clone(&stop);
        let gc_store = Arc::clone(&store);
        let gc = scope.spawn(move || {
            let mut sweeps = 0;
            while !stop_gc.load(Ordering::Relaxed) {
                gc_store.run_staging_maintenance(sweep_now).unwrap();
                sweeps += 1;
            }
            sweeps
        });
        // The writer also captures between a revision and its successor, so
        // at least one S is known to sit inside the run whatever the
        // background threads' timing.
        let mut inline = Vec::new();
        let mut extended = Vec::new();
        let mut held_after_delete = Vec::new();
        for round in 0..12 {
            let key = format!("flip{round}");
            fixture.publish("messages", &key, 1, &format!("flip {round}"));
            if round % 4 == 1 {
                let binding = fixture.binding();
                inline.push(fixture.store.capture_source_hold(&binding, long).unwrap());
            }
            fixture.publish("messages", &key, 2, &format!("flip {round} v2"));
            if round % 3 == 0 {
                let current = fixture.live_entry("messages", &key).object_id.clone();
                fixture.retire(&current);
            }
            // Every third round the writer also extends an earlier hold to
            // the tip and acknowledges through it, under the same sweeps.
            if round % 3 == 2
                && let Some(hold) = inline.get(extended.len())
            {
                let binding = fixture.binding();
                let through = fixture.store.tip().unwrap();
                let grown = fixture
                    .store
                    .extend_source_hold(&binding, &hold.hold_id, through, wide_admission())
                    .unwrap();
                fixture
                    .store
                    .acknowledge_through_source_hold(&binding, &hold.hold_id, through, 1)
                    .unwrap();
                // Acknowledged and then deleted while the sweeps run: only the
                // extension's reference keeps these bytes.
                let victim = fixture
                    .live_entry("messages", &format!("flip{}", round - 1))
                    .evidence_id
                    .clone();
                assert!(fixture.pin_refs(&hold.hold_id).contains(&victim));
                fixture.delete_evidence(&victim, ArtifactDeletionKind::Delete, deleted_at);
                held_after_delete.push(victim);
                extended.push((grown, through));
            }
        }
        stop.store(true, Ordering::Relaxed);
        let mut captured = reader.join().unwrap();
        captured.extend(inline);
        captured.sort_by_key(|hold| hold.snapshot);
        ConcurrentRun {
            captured,
            sweeps: gc.join().unwrap(),
            extended,
            held_after_delete,
        }
    });
    let ConcurrentRun {
        captured,
        sweeps,
        extended,
        held_after_delete,
    } = run;
    assert!(!held_after_delete.is_empty());
    for evidence_id in &held_after_delete {
        let digest = fixture.entry_by_evidence(evidence_id).digest.clone();
        assert!(
            fixture.object_present(&digest),
            "{evidence_id} was acknowledged, deleted, and swept, and only the extension held it"
        );
    }
    assert!(extended.len() >= 2, "extensions ran under the sweeps");
    let extended_ids: BTreeSet<&str> = extended.iter().map(|(h, _)| h.hold_id.as_str()).collect();
    for (hold, through) in &extended {
        assert_extended_hold_matches_ledger(&fixture, hold, *through);
        assert_eq!(
            fixture
                .store
                .source_hold_status(&hold.binding, &hold.hold_id, sweep_now)
                .unwrap(),
            *hold,
            "every byte an extension added under a live sweep is still on disk"
        );
    }
    assert_eq!(
        fixture.checkpoint(),
        extended.last().unwrap().1,
        "acknowledgement moved exactly to the last extended commit"
    );
    let last_publish = fixture.store.tip().unwrap();
    assert!(sweeps > 0);
    assert!(captured.len() > 3);
    assert!(
        captured
            .iter()
            .any(|hold| hold.snapshot > first_publish && hold.snapshot < last_publish),
        "some S landed inside the writer's run"
    );
    // The revision leg: some hold froze a revision 1 that a later commit superseded.
    let superseded_rev1 = captured.iter().any(|hold| {
        fixture.held_all(hold, 64).iter().any(|d| {
            d.class == "messages"
                && d.revision == 1
                && d.evidence_id.contains("-flip")
                && d.invalidated_after_snapshot.is_some()
        })
    });
    assert!(superseded_rev1, "no hold witnessed a revision in flight");
    assert!(
        captured
            .windows(2)
            .all(|pair| pair[0].snapshot <= pair[1].snapshot)
    );
    for hold in &captured {
        if extended_ids.contains(hold.hold_id.as_str()) {
            continue;
        }
        assert_hold_matches_ledger(&fixture, hold, 7);
        assert_eq!(
            fixture
                .store
                .source_hold_status(&hold.binding, &hold.hold_id, sweep_now)
                .unwrap(),
            *hold,
            "every byte captured under a live sweep is still on disk"
        );
    }
    // Holds captured after the deletion exclude the deleted evidence; the one
    // captured before still pins it, so the sweeps could not reclaim it.
    for evidence_id in &doomed {
        let digest = fixture.entry_by_evidence(evidence_id).digest.clone();
        assert!(
            fixture.object_present(&digest),
            "{evidence_id} reclaimed while pinned"
        );
        assert!(
            fixture
                .pin_refs(&pinned_before_delete.hold_id)
                .contains(evidence_id)
        );
        for hold in &captured {
            assert!(!fixture.pin_refs(&hold.hold_id).contains(evidence_id));
        }
    }
    assert_hold_matches_ledger(&fixture, &pinned_before_delete, 5);

    // Once the consumer has acknowledged every descriptor creation, releasing
    // the pin is what frees the bytes: past the grace period after release the
    // deleted evidence is reclaimed and everything else stays.
    let released_at = sweep_now;
    fixture
        .store
        .acknowledge_outbox(CONSUMER, fixture.store.tip().unwrap(), 1)
        .unwrap();
    fixture
        .store
        .release_source_hold(
            &fixture.binding(),
            &pinned_before_delete.hold_id,
            released_at,
        )
        .unwrap();
    fixture.store.run_staging_maintenance(sweep_now).unwrap();
    for evidence_id in &doomed {
        let digest = fixture.entry_by_evidence(evidence_id).digest.clone();
        assert!(
            fixture.object_present(&digest),
            "released but inside the grace period"
        );
    }
    fixture
        .store
        .run_staging_maintenance(released_at + i64::try_from(15 * DAY_MS).unwrap())
        .unwrap();
    for entry in fixture.ledger.values() {
        assert_eq!(
            fixture.object_present(&entry.digest),
            !doomed.contains(&entry.evidence_id),
            "{}",
            entry.evidence_id
        );
    }
}

#[test]
fn admission_precedes_reference_materialization_and_refusal_leaves_no_partial_hold() {
    let mut fixture = Fixture::open();
    // An empty corpus is admitted with zero references and zero bytes.
    let empty = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    assert_eq!((empty.references, empty.encoded_bytes), (0, 0));
    assert!(fixture.held_all(&empty, 4).is_empty());

    fixture.seed_five_classes();
    let (expected, references, bytes) = fixture.expected_at(fixture.store.tip().unwrap());
    assert_eq!(expected.len(), 10);
    let exact = SourceHoldBounds {
        admission: admission(references, bytes),
        max_descriptor_rows: NonZeroUsize::new(expected.len()).unwrap(),
        expiry_ms: NonZeroU64::new(HOUR_MS).unwrap(),
    };
    let refs_before = fixture.count("SELECT COUNT(*) FROM capture_pin_refs");
    let mut seen_at_admission = Vec::new();
    let hold = fixture
        .store
        .capture_source_hold_with_hook_for_test(&fixture.binding(), exact, |refs| {
            seen_at_admission.push(refs);
        })
        .unwrap();
    assert_eq!(
        seen_at_admission,
        vec![refs_before],
        "admitted before any reference row"
    );
    assert_eq!(hold.references, references);
    assert_eq!(hold.encoded_bytes, bytes);
    assert_hold_matches_ledger(&fixture, &hold, 4);

    let pins_before = fixture.count("SELECT COUNT(*) FROM capture_pins");
    let refs_before = fixture.count("SELECT COUNT(*) FROM capture_pin_refs");
    for tight in [
        SourceHoldBounds {
            admission: admission(references - 1, bytes),
            ..exact
        },
        SourceHoldBounds {
            admission: admission(references, bytes - 1),
            ..exact
        },
    ] {
        let mut seen = Vec::new();
        let error = fixture
            .store
            .capture_source_hold_with_hook_for_test(&fixture.binding(), tight, |refs| {
                seen.push(refs);
            })
            .unwrap_err();
        assert_eq!(
            error,
            SourceHoldError::Unadmitted {
                references,
                encoded_bytes: bytes,
            }
        );
        assert_eq!(seen, vec![refs_before], "refused before any reference row");
    }
    for retire_history in [false, true] {
        if retire_history {
            fixture.retire(&expected[0].object_id);
        }
        let tip = fixture.store.tip().unwrap();
        for (max_descriptor_rows, error) in [
            (
                expected.len() as u64 - 1,
                SourceHoldError::CaptureWorkLimitReached {
                    max_descriptor_rows: expected.len() - 1,
                },
            ),
            (i64::MAX as u64, SourceHoldError::InvalidRequest),
            (u64::MAX, SourceHoldError::InvalidRequest),
        ] {
            let Ok(max_descriptor_rows) = usize::try_from(max_descriptor_rows) else {
                continue;
            };
            let mut reached_reference_admission = false;
            assert_eq!(
                fixture.store.capture_source_hold_with_hook_for_test(
                    &fixture.binding(),
                    SourceHoldBounds {
                        max_descriptor_rows: NonZeroUsize::new(max_descriptor_rows).unwrap(),
                        ..exact
                    },
                    |_| reached_reference_admission = true,
                ),
                Err(error),
                "descriptor budget {max_descriptor_rows}, retired history {retire_history}"
            );
            assert!(!reached_reference_admission);
        }
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM capture_pins"),
            pins_before
        );
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM capture_pin_refs"),
            refs_before
        );
        assert_eq!(fixture.store.tip().unwrap(), tip);
        assert_eq!(fixture.checkpoint(), 0);
    }
    let after_retirement = fixture
        .store
        .capture_source_hold(&fixture.binding(), exact)
        .unwrap();
    assert_eq!(after_retirement.references, references - 1);
    assert_hold_matches_ledger(&fixture, &after_retirement, 4);
}

#[test]
fn capture_expiring_during_admission_rolls_back_pin_and_references() {
    let mut fixture = Fixture::open();
    fixture.publish("messages", "slow-capture", 1, "capture evidence");
    let lifetime_ms = 20;
    let mut seen_at_admission = false;
    let result = fixture.store.capture_source_hold_with_hook_for_test(
        &fixture.binding(),
        bounds(lifetime_ms),
        |refs| {
            seen_at_admission = true;
            assert_eq!(refs, 0);
            let expired_by = wall_ms() + i64::try_from(lifetime_ms).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(lifetime_ms + 1));
            assert!(wall_ms() >= expired_by, "the admission hook crosses expiry");
        },
    );
    assert!(seen_at_admission);
    assert_eq!(
        result,
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Expired))
    );
    assert_eq!(fixture.count("SELECT COUNT(*) FROM capture_pins"), 0);
    assert_eq!(fixture.count("SELECT COUNT(*) FROM capture_pin_refs"), 0);
    assert_eq!(fixture.checkpoint(), 0);
}

#[test]
fn maintenance_releases_expired_obligations_before_capture_admission_recovers() {
    let mut fixture = Fixture::open();
    fixture.publish("messages", "expired-cap", 1, "held until released");
    let binding = fixture.binding();
    let holds: Vec<_> = (0..MAX_ACTIVE_SOURCE_HOLDS_PER_CONSUMER)
        .map(|_| {
            fixture
                .store
                .capture_source_hold(&binding, bounds(1_000))
                .unwrap()
        })
        .collect();
    let deadline = holds.iter().map(|hold| hold.expires_at).max().unwrap();
    let remaining = u64::try_from((deadline - wall_ms()).max(0)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(remaining + 1));
    let now = wall_ms();
    assert!(now >= deadline);
    for hold in &holds {
        assert_eq!(
            fixture
                .store
                .source_hold_status(&binding, &hold.hold_id, now),
            Err(SourceHoldError::Invalid(SourceHoldInvalidity::Expired))
        );
        assert_eq!(fixture.pin_refs(&hold.hold_id).len(), 1);
    }
    assert_eq!(
        fixture.store.capture_source_hold(&binding, wide()),
        Err(SourceHoldError::HoldLimitReached)
    );
    fixture.store.run_capture_pin_maintenance(now).unwrap();
    for hold in &holds {
        assert!(fixture.pin_refs(&hold.hold_id).is_empty());
    }
    let fresh = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    assert_eq!(fresh.references, 1);
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM capture_pins WHERE released_at IS NULL"),
        1
    );
    assert_eq!(fixture.checkpoint(), 0);
}

#[test]
fn extension_expiring_during_admission_preserves_hold_and_checkpoint() {
    let mut fixture = Fixture::open();
    let binding = fixture.binding();
    let hold = fixture
        .store
        .capture_source_hold(&binding, bounds(1_000))
        .unwrap();
    fixture.publish("messages", "slow-extension", 1, "extension evidence");
    let through = fixture.store.tip().unwrap();
    let mut seen_at_admission = false;
    let result = fixture.store.extend_source_hold_with_hook_for_test(
        &binding,
        &hold.hold_id,
        through,
        wide_admission(),
        |refs| {
            seen_at_admission = true;
            assert_eq!(refs, 0);
            let remaining = u64::try_from((hold.expires_at - wall_ms()).max(0)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(remaining + 1));
            assert!(wall_ms() >= hold.expires_at);
        },
    );
    assert!(seen_at_admission);
    assert_eq!(
        result,
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Expired))
    );
    assert_eq!(fixture.count("SELECT COUNT(*) FROM capture_pins"), 1);
    assert_eq!(fixture.count("SELECT COUNT(*) FROM capture_pin_refs"), 0);
    assert_eq!(fixture.checkpoint(), 0);
    assert_eq!(
        fixture
            .store
            .source_hold_status(&binding, &hold.hold_id, hold.captured_at),
        Ok(hold)
    );
}

#[test]
fn delayed_maintenance_reclaims_from_the_stored_hold_deadline() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    let victim = fixture.held_all(&hold, 64)[0].clone();
    fixture.delete_evidence(
        &victim.evidence_id,
        ArtifactDeletionKind::Delete,
        hold.captured_at,
    );
    fixture
        .store
        .acknowledge_outbox(CONSUMER, fixture.store.tip().unwrap(), hold.captured_at)
        .unwrap();
    fixture
        .store
        .run_staging_maintenance(hold.expires_at + i64::try_from(15 * DAY_MS).unwrap())
        .unwrap();
    let released_at: i64 = fixture
        .inspect()
        .query_row(
            "SELECT released_at FROM capture_pins WHERE capture_pin_id=?1",
            [&hold.hold_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(released_at, hold.expires_at);
    assert_eq!(fixture.count("SELECT COUNT(*) FROM capture_pin_refs"), 0);
    assert!(!fixture.object_present(&victim.artifact_digest));
}

#[test]
fn a_disappearing_object_is_missing_rather_than_an_io_failure() {
    let mut fixture = Fixture::open();
    fixture.publish("messages", "vanishing", 1, "a single held object");
    let binding = fixture.binding();
    let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    let digest = fixture.held_all(&hold, 1)[0].artifact_digest.clone();
    assert!(fixture.object_present(&digest));
    assert_eq!(
        fixture.store.source_hold_status_with_hook_for_test(
            &binding,
            &hold.hold_id,
            hold.captured_at,
            |phase| {
                if phase == kernel::SourceHoldCheckPhase::BeforeObjectRead {
                    fs::remove_file(fixture.object_path(&digest)).unwrap();
                }
            },
        ),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::MissingBytes))
    );
}

#[test]
fn restored_hold_with_missing_history_fails_verification() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let hold = fixture
        .store
        .capture_source_hold(&binding, bounds(MAX_SOURCE_HOLD_LIFETIME_MS))
        .unwrap();
    let victim = fixture.held_all(&hold, 64)[0].clone();
    fixture.delete_evidence(
        &victim.evidence_id,
        ArtifactDeletionKind::Delete,
        hold.captured_at,
    );
    let destination = tempfile::tempdir().unwrap();
    fs::set_permissions(destination.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let backup = fixture
        .store
        .backup(kernel::BackupRequest {
            destination_directory: destination.path().to_path_buf(),
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
            capture_pin_expires_at: None,
        })
        .unwrap();
    assert!(!backup.evidence_refs.contains(&victim.evidence_id));
    assert!(
        fixture
            .pin_refs(&hold.hold_id)
            .contains(&victim.evidence_id)
    );
    assert!(fixture.object_present(&victim.artifact_digest));
    fixture
        .store
        .release_source_hold(&binding, &hold.hold_id, hold.captured_at)
        .unwrap();
    fixture
        .store
        .acknowledge_outbox(CONSUMER, fixture.store.tip().unwrap(), hold.captured_at)
        .unwrap();
    fixture
        .store
        .run_staging_maintenance(hold.captured_at + i64::try_from(15 * DAY_MS).unwrap())
        .unwrap();
    assert!(!fixture.object_present(&victim.artifact_digest));
    assert_eq!(
        fixture.store.restore(&backup.destination_path).unwrap(),
        backup.captured_commit_seq
    );
    assert!(
        fixture
            .pin_refs(&hold.hold_id)
            .contains(&victim.evidence_id)
    );
    assert_eq!(
        fixture
            .store
            .source_hold_status(&binding, &hold.hold_id, hold.captured_at),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::MissingBytes))
    );
}

#[test]
fn capture_work_is_bounded_independently_of_shared_evidence() {
    let mut fixture = Fixture::open();
    let text = "one shared source buffer";
    let evidence = fixture.retain("shared-work-bound", text);
    let mut objects = Vec::new();
    for index in 0..8 {
        objects.push(fixture.publish_over(
            "messages",
            &format!("shared-{index}"),
            1,
            text,
            evidence.clone(),
        ));
    }
    let binding = fixture.binding();
    let admitted = SourceHoldBounds {
        max_descriptor_rows: NonZeroUsize::new(8).unwrap(),
        admission: admission(1, text.len() as u64),
        ..wide()
    };
    for retire_history in [false, true] {
        if retire_history {
            for object in &objects[..7] {
                fixture.retire(object);
            }
        }
        let pins = fixture.count("SELECT COUNT(*) FROM capture_pins");
        let refs = fixture.count("SELECT COUNT(*) FROM capture_pin_refs");
        let tip = fixture.store.tip().unwrap();
        let mut reached_reference_admission = false;
        let result = fixture.store.capture_source_hold_with_hook_for_test(
            &binding,
            SourceHoldBounds {
                max_descriptor_rows: NonZeroUsize::new(7).unwrap(),
                ..admitted
            },
            |_| reached_reference_admission = true,
        );
        assert_eq!(
            result,
            Err(SourceHoldError::CaptureWorkLimitReached {
                max_descriptor_rows: 7
            })
        );
        assert!(!reached_reference_admission);
        assert_eq!(fixture.count("SELECT COUNT(*) FROM capture_pins"), pins);
        assert_eq!(fixture.count("SELECT COUNT(*) FROM capture_pin_refs"), refs);
        assert_eq!(fixture.store.tip().unwrap(), tip);
        assert_eq!(fixture.checkpoint(), 0);

        let hold = fixture
            .store
            .capture_source_hold(&binding, admitted)
            .unwrap();
        assert_eq!(hold.references, 1);
        assert_eq!(hold.encoded_bytes, text.len() as u64);
        assert_eq!(
            fixture.held_all(&hold, 2).len(),
            if retire_history { 1 } else { 8 }
        );
        assert_hold_matches_ledger(&fixture, &hold, 2);
    }
}

#[test]
fn backup_release_refuses_source_holds_without_changing_their_references() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    let references = fixture.pin_refs(&hold.hold_id);
    assert_eq!(
        fixture
            .store
            .release_capture_pin(&hold.hold_id, hold.captured_at),
        Err(KernelError::NotFound)
    );
    assert_eq!(fixture.pin_refs(&hold.hold_id), references);
    assert_eq!(
        fixture
            .store
            .source_hold_status(&binding, &hold.hold_id, hold.captured_at),
        Ok(hold.clone())
    );
    fixture
        .store
        .release_source_hold(&binding, &hold.hold_id, hold.captured_at)
        .unwrap();
    assert!(fixture.pin_refs(&hold.hold_id).is_empty());
}

#[test]
fn hold_status_expires_while_verifying_objects() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    assert_eq!(
        fixture.store.source_hold_status_with_hook_for_test(
            &binding,
            &hold.hold_id,
            hold.expires_at - 1,
            |phase| {
                if phase == kernel::SourceHoldCheckPhase::AfterSnapshot {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            },
        ),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Expired))
    );
}

#[test]
fn hold_status_requires_revalidation_after_extension() {
    for with_existing in [false, true] {
        for object_state in ["missing", "corrupt", "healthy"] {
            let mut fixture = Fixture::open();
            if with_existing {
                fixture.publish("messages", "existing", 1, "existing evidence");
            }
            let binding = fixture.binding();
            let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
            assert_hold_matches_ledger(&fixture, &hold, 4);
            assert_eq!(
                fixture
                    .store
                    .source_hold_status(&binding, &hold.hold_id, hold.captured_at),
                Ok(hold.clone())
            );
            let added = fixture.publish("messages", "added", 1, "added evidence");
            let through = fixture.store.tip().unwrap();
            assert!(through > hold.snapshot);
            let path = fixture.object_path(&fixture.ledger[&added].digest);
            let mut extended = hold.clone();
            let result = fixture.store.source_hold_status_with_hook_for_test(
                &binding,
                &hold.hold_id,
                hold.captured_at,
                |phase| {
                    if phase != kernel::SourceHoldCheckPhase::AfterSnapshot {
                        return;
                    }
                    match object_state {
                        "missing" => fs::remove_file(&path).unwrap(),
                        "corrupt" => fs::write(&path, b"wrong evidence").unwrap(),
                        "healthy" => {}
                        _ => unreachable!(),
                    }
                    extended = fixture
                        .store
                        .extend_source_hold(&binding, &hold.hold_id, through, wide_admission())
                        .unwrap();
                },
            );
            assert_eq!(extended.references, hold.references + 1);
            assert_extended_hold_matches_ledger(&fixture, &extended, through);
            assert!(
                result.is_err(),
                "extension requires revalidation: with_existing={with_existing}, \
                 object_state={object_state}, result={result:?}, extended={extended:?}"
            );
            assert_eq!(result, Err(SourceHoldError::VerificationChanged));
            let stable =
                fixture
                    .store
                    .source_hold_status(&binding, &hold.hold_id, hold.captured_at);
            if object_state == "healthy" {
                assert_eq!(stable, Ok(extended));
            } else {
                assert_eq!(
                    stable,
                    Err(SourceHoldError::Invalid(SourceHoldInvalidity::MissingBytes))
                );
            }
        }
    }
}

#[test]
fn hold_status_requires_revalidation_after_restore_replaces_equal_count_refs() {
    let mut fixture = Fixture::open();
    let store = Arc::clone(&fixture.store);
    let binding = fixture.binding();
    let empty = store.capture_source_hold(&binding, wide()).unwrap();
    let destination = tempfile::tempdir().unwrap();
    fs::set_permissions(destination.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let backup = store
        .backup(kernel::BackupRequest {
            destination_directory: destination.path().to_path_buf(),
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
            capture_pin_expires_at: None,
        })
        .unwrap();
    fixture.publish("messages", "before-restore", 1, "before restore");
    let hold = store
        .extend_source_hold(
            &binding,
            &empty.hold_id,
            store.tip().unwrap(),
            wide_admission(),
        )
        .unwrap();
    let original_refs = fixture.pin_refs(&hold.hold_id);
    assert_eq!(hold.references, 1);
    let result = store.source_hold_status_with_hook_for_test(
        &binding,
        &hold.hold_id,
        hold.captured_at,
        |phase| {
            if phase != kernel::SourceHoldCheckPhase::AfterSnapshot {
                return;
            }
            store.restore(&backup.destination_path).unwrap();
            assert_eq!(fixture.binding(), binding);
            assert_eq!(
                store.source_hold_status(&binding, &hold.hold_id, hold.captured_at),
                Ok(empty.clone()),
                "restore removes references without invalidating the hold"
            );
            fixture.ledger.clear();
            let added = fixture.publish("messages", "after-restore", 1, "after restore!");
            let through = store.tip().unwrap();
            let extended = store
                .extend_source_hold(&binding, &hold.hold_id, through, wide_admission())
                .unwrap();
            assert_eq!(extended, hold, "metadata and totals can remain identical");
            assert_extended_hold_matches_ledger(&fixture, &extended, through);
            let current_refs = fixture.pin_refs(&hold.hold_id);
            assert_eq!(current_refs.len(), original_refs.len());
            assert_ne!(current_refs, original_refs);
            fs::remove_file(fixture.object_path(&fixture.ledger[&added].digest)).unwrap();
        },
    );
    assert_eq!(
        store.source_hold_status(&binding, &hold.hold_id, hold.captured_at),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::MissingBytes))
    );
    assert!(
        result.is_err(),
        "different references at equal counts require revalidation: {result:?}"
    );
    assert_eq!(result, Err(SourceHoldError::VerificationChanged));
}

#[test]
fn hold_status_observes_invalidation_during_object_verification() {
    for purge in [true, false] {
        let mut fixture = Fixture::open();
        fixture.seed_five_classes();
        let binding = fixture.binding();
        let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
        let digest = fixture.held_all(&hold, 64)[0].artifact_digest.clone();
        let expected = if purge {
            SourceHoldInvalidity::PurgeDegraded
        } else {
            SourceHoldInvalidity::Released
        };
        let result = fixture.store.source_hold_status_with_hook_for_test(
            &binding,
            &hold.hold_id,
            hold.captured_at,
            |phase| {
                if phase != kernel::SourceHoldCheckPhase::AfterSnapshot {
                    return;
                }
                if purge {
                    let error = fixture
                        .store
                        .delete_artifact_with_fault_for_test(
                            ArtifactDeletionRequest {
                                intent: intent("purge-during-verification"),
                                identity: ArtifactDeletionIdentity::Digest(digest.clone()),
                                kind: ArtifactDeletionKind::Purge,
                                operator_id: Some("operator".to_string()),
                                target_locator: Some("incident://verification".to_string()),
                                reason: Some("retired".to_string()),
                                deleted_at: hold.captured_at,
                            },
                            kernel::ArtifactDeletionFault::AfterCommit,
                        )
                        .unwrap_err();
                    assert_eq!(error.kind(), kernel::ArtifactErrorKind::PurgeUnlinkPending);
                    assert!(fixture.object_present(&digest));
                } else {
                    fixture
                        .store
                        .release_source_hold(&binding, &hold.hold_id, hold.captured_at)
                        .unwrap();
                }
            },
        );
        assert_eq!(
            result,
            Err(SourceHoldError::Invalid(expected)),
            "purge={purge}"
        );
    }
}

#[test]
fn future_release_times_do_not_extend_hold_retention() {
    for removal in ["release", "deregister", "abandon", "reconcile"] {
        for maximum in [true, false] {
            let mut fixture = Fixture::open();
            fixture.seed_five_classes();
            let binding = fixture.binding();
            let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
            let released_at = if maximum {
                i64::MAX
            } else {
                hold.expires_at + 1
            };
            let release_started = wall_ms();
            match removal {
                "release" => fixture
                    .store
                    .release_source_hold(&binding, &hold.hold_id, released_at)
                    .unwrap(),
                "deregister" => {
                    fixture
                        .store
                        .acknowledge_outbox(CONSUMER, fixture.store.tip().unwrap(), 1)
                        .unwrap();
                    fixture
                        .store
                        .commit(intent("deregister-future"), |envelope| {
                            envelope.deregister_outbox_consumer(CONSUMER, released_at)?;
                            Ok(String::new())
                        })
                        .unwrap();
                }
                "abandon" => {
                    fixture
                        .store
                        .commit(intent("abandon-future"), |envelope| {
                            envelope.abandon_outbox_consumer(
                                CONSUMER,
                                kernel::ConsumerAbandonment {
                                    operator_id: "operator".to_string(),
                                    reason: "retired".to_string(),
                                    abandoned_at: released_at,
                                    barrier_id: None,
                                },
                            )?;
                            Ok(String::new())
                        })
                        .unwrap();
                }
                "reconcile" => {
                    fixture = fixture.reopen();
                    assert_eq!(
                        fixture
                            .store
                            .reconcile_source_holds(CONSUMER, released_at)
                            .unwrap(),
                        std::slice::from_ref(&hold.hold_id)
                    );
                }
                _ => unreachable!(),
            }
            let release_finished = wall_ms();
            let connection = fixture.inspect();
            let stored: i64 = connection
                .query_row(
                    "SELECT released_at FROM capture_pins WHERE capture_pin_id=?1",
                    [&hold.hold_id],
                    |row| row.get(0),
                )
                .unwrap();
            if matches!(removal, "release" | "reconcile") {
                assert_eq!(
                    stored, hold.expires_at,
                    "{removal}: release extended the hold lifetime"
                );
            } else {
                assert!(
                    (release_started..=release_finished).contains(&stored),
                    "{removal}: audit time escaped into retention: {stored}"
                );
            }
            let references: i64 = connection.query_row(
                "SELECT COUNT(*) FROM capture_pin_refs WHERE capture_pin_id=?1 AND released_at=?2",
                rusqlite::params![hold.hold_id, stored], |row| row.get(0),
            ).unwrap();
            assert_eq!(usize::try_from(references).unwrap(), hold.references);
            fixture
                .store
                .run_capture_pin_maintenance(
                    hold.expires_at + 14 * i64::try_from(DAY_MS).unwrap() + 1,
                )
                .unwrap();
            assert_eq!(fixture.count("SELECT COUNT(*) FROM capture_pin_refs"), 0);
        }
    }
}

#[test]
fn repeated_purges_preserve_the_first_hold_degradation() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    let descriptors = fixture.held_all(&hold, 64);
    let first = &descriptors[0];
    let second = descriptors
        .iter()
        .find(|descriptor| descriptor.artifact_digest != first.artifact_digest)
        .unwrap();
    let degradation = |fixture: &Fixture| -> (i64, String) {
        fixture
            .inspect()
            .query_row(
                "SELECT purge_degraded_at,purge_barrier_id FROM capture_pins
                 WHERE capture_pin_id=?1",
                [&hold.hold_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    };

    let first_commit = fixture.delete_evidence(&first.evidence_id, ArtifactDeletionKind::Purge, 42);
    let first_degradation = degradation(&fixture);
    assert_eq!(first_degradation.0, 42);
    let second_commit =
        fixture.delete_evidence(&second.evidence_id, ArtifactDeletionKind::Purge, 43);
    assert!(second_commit > first_commit);
    assert_eq!(degradation(&fixture), first_degradation);
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM artifact_purge_tombstones"),
        2
    );
    assert_eq!(fixture.pin_refs(&hold.hold_id).len(), hold.references);
    assert_eq!(
        fixture
            .store
            .source_hold_status(&binding, &hold.hold_id, hold.captured_at),
        Err(SourceHoldError::Invalid(
            SourceHoldInvalidity::PurgeDegraded
        ))
    );

    let fresh = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    assert_eq!(fresh.references + 2, hold.references);
    assert_hold_matches_ledger(&fixture, &fresh, 4);
}

#[test]
fn purge_expiry_missing_bytes_and_release_invalidate_the_hold_without_moving_the_consumer() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let checkpoint = fixture.store.tip().unwrap();
    fixture
        .store
        .acknowledge_outbox(CONSUMER, checkpoint, 1)
        .unwrap();
    assert!(checkpoint > 0);
    assert_eq!(fixture.checkpoint(), checkpoint);
    let status = |fixture: &Fixture, hold: &SourceHold, now: i64| {
        fixture
            .store
            .source_hold_status(&binding, &hold.hold_id, now)
    };

    // Expiry: judged against the caller's clock, and the maintenance sweep
    // releases the expired pin durably. A dead hold serves no page.
    let expiring = fixture
        .store
        .capture_source_hold(&binding, bounds(HOUR_MS / 2))
        .unwrap();
    let fresh = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    assert!(
        fixture
            .first_page(&binding, &expiring.hold_id, expiring.expires_at - 1)
            .is_ok()
    );
    assert_eq!(
        status(&fixture, &expiring, expiring.expires_at),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Expired))
    );
    assert_eq!(
        fixture.first_page(&binding, &expiring.hold_id, expiring.expires_at),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Expired))
    );
    assert_eq!(
        status(&fixture, &fresh, expiring.expires_at),
        Ok(fresh.clone())
    );
    fixture
        .store
        .run_capture_pin_maintenance(expiring.expires_at + 1)
        .unwrap();
    assert_eq!(
        status(&fixture, &expiring, 0),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Released))
    );
    assert_eq!(
        status(&fixture, &fresh, expiring.expires_at),
        Ok(fresh.clone())
    );

    // Purge degradation: purging held evidence marks the pin degraded, and a
    // hold captured afterwards does not list the purged descriptor.
    let held = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    let victim = fixture.held_all(&held, 64)[0].clone();
    fixture.delete_evidence(&victim.evidence_id, ArtifactDeletionKind::Purge, 42);
    assert_eq!(
        status(&fixture, &held, held.captured_at),
        Err(SourceHoldError::Invalid(
            SourceHoldInvalidity::PurgeDegraded
        ))
    );
    assert_eq!(
        fixture.first_page(&binding, &held.hold_id, held.captured_at),
        Err(SourceHoldError::Invalid(
            SourceHoldInvalidity::PurgeDegraded
        ))
    );
    let after_purge = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    assert_hold_matches_ledger(&fixture, &after_purge, 4);
    assert!(
        !fixture
            .held_all(&after_purge, 64)
            .iter()
            .any(|d| d.evidence_id == victim.evidence_id)
    );
    assert_eq!(after_purge.references + 1, held.references);

    // Missing bytes: an object removed from disk behind a valid pin.
    assert_eq!(
        status(&fixture, &after_purge, after_purge.captured_at),
        Ok(after_purge.clone())
    );
    let gone = fixture.held_all(&after_purge, 64)[0].clone();
    let gone_path = fixture.object_path(&gone.artifact_digest);
    // A shard that cannot be probed is an I/O failure, not `MissingBytes`: the
    // hold still protects its bytes.
    let shard = gone_path.parent().unwrap();
    fs::set_permissions(shard, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::metadata(&gone_path).is_ok() {
        // A privileged test process ignores the mode bits.
        fs::set_permissions(shard, fs::Permissions::from_mode(0o700)).unwrap();
    } else {
        assert_eq!(
            status(&fixture, &after_purge, after_purge.captured_at),
            Err(SourceHoldError::Kernel(KernelError::Io))
        );
        fs::set_permissions(shard, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            status(&fixture, &after_purge, after_purge.captured_at),
            Ok(after_purge.clone())
        );
    }
    let original = fs::read(&gone_path).unwrap();
    let mut overwritten = original.clone();
    overwritten[0] ^= 1;
    for damaged in [&original[..original.len() - 1], overwritten.as_slice()] {
        fs::write(&gone_path, damaged).unwrap();
        assert_eq!(
            status(&fixture, &after_purge, after_purge.captured_at),
            Err(SourceHoldError::Invalid(SourceHoldInvalidity::MissingBytes))
        );
        fs::write(&gone_path, &original).unwrap();
        assert_eq!(
            status(&fixture, &after_purge, after_purge.captured_at),
            Ok(after_purge.clone())
        );
    }
    fs::set_permissions(&gone_path, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&gone_path).is_err() {
        assert_eq!(
            status(&fixture, &after_purge, after_purge.captured_at),
            Err(SourceHoldError::Kernel(KernelError::Io))
        );
    }
    fs::set_permissions(&gone_path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_file(&gone_path).unwrap();
    assert_eq!(
        status(&fixture, &after_purge, after_purge.captured_at),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::MissingBytes))
    );

    // Release is idempotent for an existing hold and an error for a missing one.
    let released = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    fixture
        .store
        .release_source_hold(&binding, &released.hold_id, 7)
        .unwrap();
    fixture
        .store
        .release_source_hold(&binding, &released.hold_id, 8)
        .unwrap();
    assert_eq!(
        status(&fixture, &released, 0),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Released))
    );
    assert_eq!(
        fixture.first_page(&binding, &released.hold_id, 0),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Released))
    );
    assert!(fixture.pin_refs(&released.hold_id).is_empty());
    assert_eq!(
        fixture
            .store
            .source_hold_status(&binding, "no-such-hold", 0),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Missing))
    );
    assert_eq!(
        fixture
            .store
            .release_source_hold(&binding, "no-such-hold", 7),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Missing))
    );
    // Nothing above acknowledged, rewound, or abandoned the consumer.
    assert_eq!(fixture.checkpoint(), checkpoint);
    assert_eq!(fixture.count("SELECT COUNT(*) FROM outbox_consumers"), 1);
}

#[test]
fn a_new_incarnation_reconciles_old_holds_and_captures_a_new_s() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let old_binding = fixture.binding();
    let long = bounds(MAX_SOURCE_HOLD_LIFETIME_MS);
    let old = fixture
        .store
        .capture_source_hold(&old_binding, long)
        .unwrap();
    let older = fixture
        .store
        .capture_source_hold(&old_binding, long)
        .unwrap();
    let old_inventory = fixture.held_all(&old, 64);
    // Another consumer's hold in the old incarnation is not this consumer's to reconcile.
    fixture
        .store
        .commit(intent("other-consumer"), |envelope| {
            envelope.register_outbox_consumer("other", 1)?;
            Ok(String::new())
        })
        .unwrap();
    let other_binding = SourceHoldBinding {
        consumer_id: "other".to_string(),
        ..old_binding.clone()
    };
    let other = fixture
        .store
        .capture_source_hold(&other_binding, long)
        .unwrap();
    // Evidence deleted after the holds took S stays pinned by them alone,
    // until every one of them is released.
    let doomed = fixture.live_entry("git_commits", "a").evidence_id.clone();
    let deleted_at = wall_ms();
    fixture.delete_evidence(&doomed, ArtifactDeletionKind::Delete, deleted_at);

    // One catch-up window is acknowledged through the hold, then the process
    // is cut after the next extension and before its acknowledgement.
    fixture.publish("messages", "landed", 1, "acknowledged before the cut");
    let landed_through = fixture.store.tip().unwrap();
    fixture
        .store
        .extend_source_hold(&old_binding, &old.hold_id, landed_through, wide_admission())
        .unwrap();
    fixture
        .store
        .acknowledge_through_source_hold(&old_binding, &old.hold_id, landed_through, 1)
        .unwrap();
    fixture.publish("messages", "cut", 1, "cut before acknowledgement");
    let cut_through = fixture.store.tip().unwrap();
    let extended_before_cut = fixture
        .store
        .extend_source_hold(&old_binding, &old.hold_id, cut_through, wide_admission())
        .unwrap();
    let checkpoint_before_cut = fixture.checkpoint();
    assert_eq!(
        checkpoint_before_cut, landed_through,
        "no progress is lost either"
    );

    let mut fixture = fixture.reopen();
    let new_binding = fixture.binding();
    assert_ne!(new_binding.lease_epoch, old_binding.lease_epoch);
    // No progress is invented for the cut extension: the old hold cannot
    // acknowledge under the new incarnation, and the checkpoint stands.
    assert_eq!(
        fixture
            .store
            .acknowledge_through_source_hold(&new_binding, &old.hold_id, cut_through, 1)
            .unwrap_err(),
        SourceHoldError::BindingMismatch
    );
    assert_eq!(fixture.checkpoint(), checkpoint_before_cut);
    assert_eq!(
        fixture.pin_refs(&old.hold_id).len(),
        extended_before_cut.references,
        "the cut extension's references survive until reconciliation"
    );

    assert_eq!(
        fixture
            .store
            .source_hold_status(&old_binding, &old.hold_id, old.captured_at),
        Err(SourceHoldError::IncarnationMismatch)
    );
    assert_eq!(
        fixture.first_page(&old_binding, &old.hold_id, old.captured_at),
        Err(SourceHoldError::IncarnationMismatch)
    );
    assert_eq!(
        fixture
            .store
            .release_source_hold(&old_binding, &old.hold_id, old.captured_at),
        Err(SourceHoldError::IncarnationMismatch)
    );

    // The old hold cannot be used under the new incarnation.
    assert_eq!(
        fixture
            .store
            .source_hold_status(&new_binding, &old.hold_id, old.captured_at)
            .unwrap_err(),
        SourceHoldError::BindingMismatch
    );
    let released_at = wall_ms();
    let mut released = fixture
        .store
        .reconcile_source_holds(CONSUMER, released_at)
        .unwrap();
    released.sort();
    let mut expected = vec![old.hold_id.clone(), older.hold_id.clone()];
    expected.sort();
    assert_eq!(released, expected);
    assert_eq!(
        fixture.checkpoint(),
        checkpoint_before_cut,
        "reconciliation closes holds; it never completes their acknowledgement"
    );
    assert!(
        fixture
            .store
            .reconcile_source_holds(CONSUMER, released_at)
            .unwrap()
            .is_empty()
    );
    // The old binding itself is refused in the new incarnation; the pins are
    // observed released through the tables.
    for hold in [&old, &older] {
        assert_eq!(
            fixture
                .store
                .source_hold_status(&old_binding, &hold.hold_id, 0),
            Err(SourceHoldError::IncarnationMismatch)
        );
        assert!(fixture.pin_released(&hold.hold_id));
        assert!(
            fixture.pin_refs(&hold.hold_id).is_empty(),
            "references released with the pin"
        );
    }
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM capture_pins WHERE released_at IS NOT NULL"),
        2
    );
    // The other consumer's old hold is untouched and still pins the deleted evidence.
    assert!(!fixture.pin_released(&other.hold_id));
    assert_eq!(
        fixture
            .store
            .source_hold_status(&other_binding, &other.hold_id, other.captured_at),
        Err(SourceHoldError::IncarnationMismatch)
    );
    assert!(fixture.pin_refs(&other.hold_id).contains(&doomed));

    // A new S is captured fresh; the old cursor is not resumed, and the
    // deleted evidence is not part of it.
    fixture.publish("messages", "after-reopen", 1, "after reopen");
    let fresh = fixture
        .store
        .capture_source_hold(&new_binding, long)
        .unwrap();
    assert!(fresh.snapshot > old.snapshot);
    assert_hold_matches_ledger(&fixture, &fresh, 4);
    assert!(!fixture.pin_refs(&fresh.hold_id).contains(&doomed));
    // Immutable bytes survived the reopen and reconciliation.
    for descriptor in &old_inventory {
        let entry = fixture.entry_by_evidence(&descriptor.evidence_id);
        assert_eq!(
            fs::read(fixture.object_path(&descriptor.artifact_digest)).unwrap(),
            entry.text.as_bytes()
        );
    }
    // Reconciliation freed protection: once the other consumer's pin is also
    // released, both consumers have acknowledged the descriptor creations, and
    // the grace period passes, the deleted evidence is reclaimed while
    // everything the fresh hold cites stays.
    fixture
        .store
        .reconcile_source_holds("other", released_at)
        .unwrap();
    let acknowledged = fixture.store.tip().unwrap();
    for consumer in [CONSUMER, "other"] {
        fixture
            .store
            .acknowledge_outbox(consumer, acknowledged, 1)
            .unwrap();
    }
    let doomed_digest = fixture.entry_by_evidence(&doomed).digest.clone();
    fixture
        .store
        .run_staging_maintenance(released_at + i64::try_from(15 * DAY_MS).unwrap())
        .unwrap();
    assert!(!fixture.object_present(&doomed_digest));
    assert_eq!(
        fixture
            .store
            .source_hold_status(&new_binding, &fresh.hold_id, fresh.captured_at),
        Ok(fresh.clone())
    );

    // A name-only remediation rewrites the domain name and changes no held input.
    let before_name = fixture.domain_name();
    let receipt = fixture
        .store
        .commit(intent("remediate"), |envelope| {
            envelope.remediate_text(
                RemediationTarget::CanonicalDomainName {
                    object_id: "domain-object".to_string(),
                },
                "operator",
                5,
            )?;
            Ok(String::new())
        })
        .unwrap();
    assert!(receipt.commit_seq > fresh.snapshot);
    assert_ne!(
        fixture.domain_name(),
        before_name,
        "the remediation changed the name"
    );
    assert_hold_matches_ledger(&fixture, &fresh, 4);
    assert_eq!(
        fixture
            .store
            .source_hold_status(&new_binding, &fresh.hold_id, fresh.captured_at),
        Ok(fresh.clone())
    );
}

#[test]
fn replay_evidence_created_after_s_survives_publication_pruning_and_gc_until_acknowledged() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    // A second, slower consumer: the replay horizon is the least advanced
    // checkpoint, never the most advanced one.
    fixture
        .store
        .commit(intent("lagging-consumer"), |envelope| {
            envelope.register_outbox_consumer("lagging", 1)?;
            Ok(String::new())
        })
        .unwrap();
    let acknowledged = fixture.store.tip().unwrap();
    for consumer in [CONSUMER, "lagging"] {
        fixture
            .store
            .acknowledge_outbox(consumer, acknowledged, 1)
            .unwrap();
    }
    let hold = fixture
        .store
        .capture_source_hold(&binding, bounds(MAX_SOURCE_HOLD_LIFETIME_MS))
        .unwrap();
    let far = wall_ms() + i64::try_from(15 * DAY_MS).unwrap();

    // An independent after-S ledger: creations, a revision, a retirement, and
    // logical deletions of the evidence behind two of them.
    fixture.publish("messages", "late", 1, "late message");
    fixture.publish("messages", "late", 2, "late message v2");
    fixture.publish("canonical_claims", "late", 1, "late claim");
    fixture.publish("raw_tool_spans", "late", 1, "late tool span");
    let retired = fixture
        .live_entry("canonical_claims", "late")
        .object_id
        .clone();
    fixture.retire(&retired);
    let after_s: Vec<LedgerEntry> = fixture
        .ledger
        .values()
        .filter(|entry| entry.created > hold.snapshot)
        .cloned()
        .collect();
    assert_eq!(after_s.len(), 4);
    let deleted: Vec<String> = [
        "evidence-messages-late-1",
        "evidence-canonical_claims-late-1",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    for evidence_id in &deleted {
        fixture.delete_evidence(evidence_id, ArtifactDeletionKind::Delete, wall_ms());
    }
    assert_hold_matches_ledger(&fixture, &hold, 4);

    // Publication and pruning do not release anything; only checkpoints do.
    fixture.publish_and_prune();
    fixture.store.run_staging_maintenance(far).unwrap();
    for entry in &after_s {
        assert_eq!(
            fs::read(fixture.object_path(&entry.digest)).unwrap(),
            entry.text.as_bytes(),
            "{} replay bytes intact before acknowledgement",
            entry.evidence_id
        );
    }
    assert_eq!(fixture.checkpoint(), acknowledged);

    // Negative control: acknowledging without extending the hold lets the
    // deleted after-S evidence go once the grace period passes, but only once
    // the slowest consumer has acknowledged past each creation.
    let tip = fixture.store.tip().unwrap();
    fixture.store.acknowledge_outbox(CONSUMER, tip, 1).unwrap();
    let swept = fixture.store.run_staging_maintenance(far).unwrap();
    assert_eq!(
        swept.artifact_gc.withheld_for_replay, 2,
        "both deleted after-S artifacts are reported as withheld by the lagging consumer"
    );
    for entry in &after_s {
        assert!(
            fixture.object_present(&entry.digest),
            "{} is kept while the lagging consumer has not acknowledged it",
            entry.evidence_id
        );
    }
    // The horizon is inclusive: a creation acknowledged exactly is released,
    // a later one is not.
    let first_deleted = fixture.entry_by_evidence(&deleted[0]).clone();
    let second_deleted = fixture.entry_by_evidence(&deleted[1]).clone();
    assert!(first_deleted.created < second_deleted.created);
    fixture
        .store
        .acknowledge_outbox("lagging", first_deleted.created, 1)
        .unwrap();
    let swept = fixture.store.run_staging_maintenance(far).unwrap();
    assert_eq!(swept.artifact_gc.withheld_for_replay, 1);
    assert!(!fixture.object_present(&first_deleted.digest));
    assert!(fixture.object_present(&second_deleted.digest));
    fixture.store.acknowledge_outbox("lagging", tip, 1).unwrap();
    let swept = fixture.store.run_staging_maintenance(far).unwrap();
    assert_eq!(swept.artifact_gc.withheld_for_replay, 0);
    for entry in &after_s {
        assert_eq!(
            fixture.object_present(&entry.digest),
            !deleted.contains(&entry.evidence_id),
            "{}",
            entry.evidence_id
        );
    }
    assert_eq!(
        fixture
            .store
            .source_hold_status(&binding, &hold.hold_id, hold.captured_at),
        Ok(hold.clone()),
        "the hold at S is untouched by after-S collection"
    );

    // Extension before acknowledgement keeps the catch-up bytes through
    // acknowledgement and publication, until the hold is released.
    fixture.publish("promoted_memory", "catchup", 1, "catch-up memory");
    fixture.publish("git_commits", "catchup", 1, "catch-up commit");
    let catchup: Vec<LedgerEntry> = fixture
        .ledger
        .values()
        .filter(|entry| entry.created > tip)
        .cloned()
        .collect();
    assert_eq!(catchup.len(), 2);
    let through = fixture.store.tip().unwrap();
    let extended = fixture
        .store
        .extend_source_hold(&binding, &hold.hold_id, through, wide_admission())
        .unwrap();
    assert_eq!(
        extended.expires_at, hold.expires_at,
        "extension never renews expiry"
    );
    assert_eq!(extended.snapshot, hold.snapshot);
    assert_extended_hold_matches_ledger(&fixture, &extended, through);
    fixture
        .store
        .acknowledge_through_source_hold(&binding, &hold.hold_id, through, 1)
        .unwrap();
    assert_eq!(fixture.checkpoint(), through);
    // The acknowledged evidence is deleted afterwards: without the hold the
    // consumer's checkpoint no longer protects it, so only the pin does.
    fixture.delete_evidence(
        &catchup[0].evidence_id,
        ArtifactDeletionKind::Delete,
        wall_ms(),
    );
    for consumer in [CONSUMER, "lagging"] {
        fixture
            .store
            .acknowledge_outbox(consumer, fixture.store.tip().unwrap(), 1)
            .unwrap();
    }
    fixture.publish_and_prune();
    fixture.store.run_staging_maintenance(far).unwrap();
    for entry in &catchup {
        assert_eq!(
            fs::read(fixture.object_path(&entry.digest)).unwrap(),
            entry.text.as_bytes(),
            "{} survives acknowledgement while held",
            entry.evidence_id
        );
    }
    let released_at = far;
    fixture
        .store
        .release_source_hold(&binding, &hold.hold_id, released_at)
        .unwrap();
    fixture
        .store
        .run_staging_maintenance(released_at + i64::try_from(15 * DAY_MS).unwrap())
        .unwrap();
    assert!(
        !fixture.object_present(&catchup[0].digest),
        "released and acknowledged: collected"
    );
    assert!(
        fixture.object_present(&catchup[1].digest),
        "live evidence stays"
    );
}

#[test]
fn natural_reclaim_recovery_keeps_unacknowledged_descriptor_evidence() {
    let mut fixture = Fixture::open();
    fixture.publish("messages", "reclaiming", 1, "replay after reopen");
    let evidence = fixture.live_entry("messages", "reclaiming").clone();
    fixture.delete_evidence(
        &evidence.evidence_id,
        ArtifactDeletionKind::Delete,
        wall_ms(),
    );
    let checkpoint = fixture.checkpoint();
    assert!(checkpoint < evidence.created);
    assert_eq!(fixture.count("SELECT COUNT(*) FROM capture_pin_refs"), 0);
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM artifact_pending_unlinks"),
        0
    );
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM artifact_ingestion_reservations"),
        0
    );

    let far = wall_ms() + i64::try_from(15 * DAY_MS).unwrap();
    let connection = Connection::open(fixture.root.path().join("kernel.sqlite")).unwrap();
    assert_eq!(
        connection
            .execute(
                "INSERT INTO artifact_ingestion_reservations(
                     reservation_id,artifact_digest,artifact_reference,state,writer_epoch,
                     created_at,heartbeat_at,lease_expires_at,reclaim_started_at
                 ) SELECT 'gc-'||e.artifact_digest,e.artifact_digest,e.artifact_reference,
                          'Reclaiming',?1,?2,?2,?2,?2
                   FROM evidence_meta e JOIN commit_log c ON c.commit_seq=e.invalidated_commit_seq
                   WHERE e.evidence_id=?3 AND e.retain_until IS NULL AND c.recorded_at+?4<=?2",
                rusqlite::params![
                    i64::try_from(fixture.store.lease_epoch()).unwrap(),
                    far,
                    evidence.evidence_id,
                    i64::try_from(14 * DAY_MS).unwrap(),
                ],
            )
            .unwrap(),
        1,
        "the stored reclaim decision has passed the reference grace"
    );
    drop(connection);
    assert_eq!(
        fs::read(fixture.object_path(&evidence.digest)).unwrap(),
        evidence.text.as_bytes()
    );

    let fixture = fixture.reopen();
    assert_eq!(fixture.checkpoint(), checkpoint);
    assert_eq!(
        fs::read(fixture.object_path(&evidence.digest))
            .ok()
            .as_deref(),
        Some(evidence.text.as_bytes()),
        "startup recovery must retain unacknowledged descriptor evidence"
    );
    let swept = fixture.store.run_staging_maintenance(far).unwrap();
    assert_eq!(swept.artifact_gc.reclaimed_objects, 0);
    assert_eq!(swept.artifact_gc.withheld_for_replay, 1);
    assert_eq!(
        fixture
            .count("SELECT COUNT(*) FROM artifact_ingestion_reservations WHERE state='Reclaiming'"),
        0
    );

    fixture
        .store
        .acknowledge_outbox(CONSUMER, evidence.created, 1)
        .unwrap();
    let swept = fixture.store.run_staging_maintenance(far).unwrap();
    assert_eq!(swept.artifact_gc.withheld_for_replay, 0);
    assert_eq!(swept.artifact_gc.reclaimed_objects, 1);
    assert!(!fixture.object_present(&evidence.digest));
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM artifact_ingestion_reservations"),
        0
    );
}

#[test]
fn natural_reclaim_rechecks_historical_hold_extensions_and_purge_overrides_them() {
    let mut fixture = Fixture::open();
    let binding = fixture.binding();
    let hold = fixture
        .store
        .capture_source_hold(&binding, bounds(MAX_SOURCE_HOLD_LIFETIME_MS))
        .unwrap();
    fixture.publish("messages", "held-reclaim", 1, "held historical evidence");
    let evidence = fixture.live_entry("messages", "held-reclaim").clone();
    fixture.delete_evidence(
        &evidence.evidence_id,
        ArtifactDeletionKind::Delete,
        wall_ms(),
    );
    fixture
        .store
        .acknowledge_outbox(CONSUMER, evidence.created, 1)
        .unwrap();
    let far = wall_ms() + i64::try_from(15 * DAY_MS).unwrap();
    assert_eq!(
        fixture
            .store
            .run_staging_maintenance_with_fault_for_test(far, ArtifactGcFault::AfterReclaiming),
        Err(KernelError::Fault)
    );
    assert_eq!(
        fixture
            .count("SELECT COUNT(*) FROM artifact_ingestion_reservations WHERE state='Reclaiming'"),
        1
    );
    let extended = fixture
        .store
        .extend_source_hold(&binding, &hold.hold_id, evidence.created, wide_admission())
        .unwrap();
    assert_eq!(
        fixture.pin_refs(&hold.hold_id),
        std::slice::from_ref(&evidence.evidence_id)
    );
    fixture
        .store
        .commit(intent("new-lagging-consumer"), |envelope| {
            envelope.register_outbox_consumer("lagging", 1)?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        fixture.count(
            "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id='lagging'"
        ),
        0
    );

    let swept = fixture.store.run_staging_maintenance(far).unwrap();
    assert_eq!(
        swept.artifact_gc.reclaimed_objects, 0,
        "a historical hold extension protects an already-reclaiming artifact"
    );
    assert_eq!(swept.artifact_gc.withheld_for_replay, 0);
    assert_eq!(
        fixture
            .store
            .source_hold_status(&binding, &hold.hold_id, wall_ms()),
        Ok(extended)
    );
    let (_, readmitted_digest) = fixture.retain("readmitted-reclaim", &evidence.text);
    assert_eq!(readmitted_digest, evidence.digest);
    assert_eq!(
        fs::read(fixture.object_path(&readmitted_digest)).unwrap(),
        evidence.text.as_bytes()
    );

    fixture
        .store
        .delete_artifact_with_fault_for_test(
            ArtifactDeletionRequest {
                intent: intent("purge-held-reclaim"),
                identity: ArtifactDeletionIdentity::Digest(evidence.digest.clone()),
                kind: ArtifactDeletionKind::Purge,
                operator_id: Some("operator-1".to_string()),
                target_locator: Some("incident://1".to_string()),
                reason: Some("secret".to_string()),
                deleted_at: wall_ms(),
            },
            ArtifactDeletionFault::AfterCommit,
        )
        .unwrap_err();
    assert!(fixture.object_present(&evidence.digest));
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM artifact_pending_unlinks"),
        1
    );
    let fixture = fixture.reopen();
    assert!(!fixture.object_present(&evidence.digest));
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM artifact_pending_unlinks"),
        0
    );
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM artifact_ingestion_reservations"),
        0
    );
    assert_eq!(fixture.checkpoint(), evidence.created);
    assert_eq!(
        fixture.count(
            "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id='lagging'"
        ),
        0
    );
}

#[test]
fn reclaim_cleanup_retires_absent_objects_despite_replay_protection() {
    for active_pin in [false, true] {
        let mut fixture = Fixture::open();
        let binding = fixture.binding();
        let hold = fixture
            .store
            .capture_source_hold(&binding, bounds(MAX_SOURCE_HOLD_LIFETIME_MS))
            .unwrap();
        fixture.publish("messages", "unlinked", 1, "unlinked evidence");
        let evidence = fixture.live_entry("messages", "unlinked").clone();
        fixture.delete_evidence(
            &evidence.evidence_id,
            ArtifactDeletionKind::Delete,
            wall_ms(),
        );
        fixture
            .store
            .acknowledge_outbox(CONSUMER, evidence.created, 1)
            .unwrap();
        let far = wall_ms() + i64::try_from(15 * DAY_MS).unwrap();
        assert_eq!(
            fixture
                .store
                .run_staging_maintenance_with_fault_for_test(far, ArtifactGcFault::AfterUnlink),
            Err(KernelError::Fault)
        );
        assert!(!fixture.object_present(&evidence.digest));
        assert_eq!(
            fixture.count(
                "SELECT COUNT(*) FROM artifact_ingestion_reservations WHERE state='Reclaiming'"
            ),
            1
        );
        if active_pin {
            fixture
                .store
                .extend_source_hold(&binding, &hold.hold_id, evidence.created, wide_admission())
                .unwrap();
            assert_eq!(
                fixture.pin_refs(&hold.hold_id),
                std::slice::from_ref(&evidence.evidence_id)
            );
        } else {
            fixture
                .store
                .commit(intent("lagging-after-unlink"), |envelope| {
                    envelope.register_outbox_consumer("lagging", 1)?;
                    Ok(String::new())
                })
                .unwrap();
            assert_eq!(
                fixture.count(
                    "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id='lagging'"
                ),
                0
            );
        }

        let swept = fixture.store.run_staging_maintenance(far).unwrap();
        assert_eq!(swept.artifact_gc.reclaimed_objects, 0);
        assert_eq!(swept.artifact_gc.reclaimed_bytes, 0);
        assert_eq!(swept.artifact_gc.failed_candidates, 0);
        assert_eq!(
            swept.artifact_gc.withheld_for_replay, 0,
            "absent bytes cannot be withheld for replay"
        );
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM artifact_ingestion_reservations"),
            0,
            "completed unlinks retire their reclaim notes despite active pins"
        );
        let fixture = fixture.reopen();
        assert!(!fixture.object_present(&evidence.digest));
        assert_eq!(fixture.checkpoint(), evidence.created);
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM artifact_ingestion_reservations"),
            0
        );
    }
}

#[test]
fn reclaim_cleanup_does_not_treat_an_unreadable_object_as_absent() {
    let mut fixture = Fixture::open();
    fixture.publish(
        "messages",
        "unreadable-reclaim",
        1,
        "protected unreadable bytes",
    );
    let evidence = fixture.live_entry("messages", "unreadable-reclaim").clone();
    fixture.delete_evidence(
        &evidence.evidence_id,
        ArtifactDeletionKind::Delete,
        wall_ms(),
    );
    fixture
        .store
        .acknowledge_outbox(CONSUMER, evidence.created, 1)
        .unwrap();
    let far = wall_ms() + i64::try_from(15 * DAY_MS).unwrap();
    assert_eq!(
        fixture
            .store
            .run_staging_maintenance_with_fault_for_test(far, ArtifactGcFault::AfterReclaiming),
        Err(KernelError::Fault)
    );
    fixture
        .store
        .commit(intent("lagging-with-unreadable-object"), |envelope| {
            envelope.register_outbox_consumer("lagging", 1)?;
            Ok(String::new())
        })
        .unwrap();

    let object = fixture.object_path(&evidence.digest);
    let shard = object.parent().unwrap();
    fs::set_permissions(shard, fs::Permissions::from_mode(0o000)).unwrap();
    let probe_error = fs::metadata(&object).err();
    let swept = fixture.store.run_staging_maintenance(far);
    fs::set_permissions(shard, fs::Permissions::from_mode(0o700)).unwrap();
    if let Some(error) = probe_error {
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        eprintln!("permission-denied object probe exercised");
    } else {
        eprintln!("privileged process bypassed mode bits; unreadable probe not exercised");
    }
    let swept = swept.unwrap();
    assert_eq!(swept.artifact_gc.withheld_for_replay, 1);
    assert_eq!(swept.artifact_gc.failed_candidates, 0);
    assert_eq!(swept.artifact_gc.reclaimed_objects, 0);
    assert_eq!(fs::read(object).unwrap(), evidence.text.as_bytes());
}

#[test]
fn extension_is_bounded_idempotent_and_gates_acknowledgement() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let checkpoint = fixture.store.tip().unwrap();
    fixture
        .store
        .acknowledge_outbox(CONSUMER, checkpoint, 1)
        .unwrap();
    let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    // A descriptor created and superseded inside the window is still replay
    // evidence; the batch also cites one artifact from two descriptors.
    fixture.publish("messages", "w", 1, "window message");
    fixture.publish("messages", "w", 2, "window message v2");
    let shared = fixture.retain("window-shared", "window shared bytes");
    fixture.publish_over(
        "promoted_memory",
        "w",
        1,
        "window shared bytes",
        shared.clone(),
    );
    fixture.publish_over("git_commits", "w", 1, "window shared bytes", shared);
    let through = fixture.store.tip().unwrap();
    let (expected_refs, expected_bytes) = fixture.expected_refs_through(hold.snapshot, through);
    let added = expected_refs.len() - hold.references;
    assert_eq!(added, 3, "two window messages and one shared artifact");

    // Acknowledgement is refused until the hold covers the window.
    assert_eq!(
        fixture
            .store
            .acknowledge_through_source_hold(&binding, &hold.hold_id, through, 1),
        Err(SourceHoldError::ExtensionIncomplete { uncovered: added })
    );
    assert_eq!(fixture.checkpoint(), checkpoint);

    // Admission is judged on the whole hold before any reference is written.
    let refs_before = fixture.count("SELECT COUNT(*) FROM capture_pin_refs");
    let mut seen = Vec::new();
    let error = fixture
        .store
        .extend_source_hold_with_hook_for_test(
            &binding,
            &hold.hold_id,
            through,
            admission(expected_refs.len() - 1, expected_bytes),
            |refs| seen.push(refs),
        )
        .unwrap_err();
    assert_eq!(
        error,
        SourceHoldError::Unadmitted {
            references: expected_refs.len(),
            encoded_bytes: expected_bytes,
        }
    );
    assert_eq!(seen, vec![refs_before]);
    assert_eq!(
        fixture
            .store
            .extend_source_hold(
                &binding,
                &hold.hold_id,
                through,
                admission(expected_refs.len(), expected_bytes - 1)
            )
            .unwrap_err(),
        SourceHoldError::Unadmitted {
            references: expected_refs.len(),
            encoded_bytes: expected_bytes,
        }
    );
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM capture_pin_refs"),
        refs_before
    );
    assert_eq!(
        fixture
            .store
            .acknowledge_through_source_hold(&binding, &hold.hold_id, through, 1),
        Err(SourceHoldError::ExtensionIncomplete { uncovered: added }),
        "a failed extension authorizes nothing"
    );
    assert_eq!(fixture.checkpoint(), checkpoint);

    // The window must lie within [S, tip]; the binding must match.
    for bad in [hold.snapshot - 1, through + 1] {
        assert_eq!(
            fixture
                .store
                .extend_source_hold(&binding, &hold.hold_id, bad, wide_admission())
                .unwrap_err(),
            SourceHoldError::InvalidRequest,
            "{bad}"
        );
    }
    let mut other = binding.clone();
    other.source_policy_version = "source-policy.v2".to_string();
    assert_eq!(
        fixture
            .store
            .extend_source_hold(&other, &hold.hold_id, through, wide_admission())
            .unwrap_err(),
        SourceHoldError::BindingMismatch
    );

    // The admitted extension equals the ledger; replaying it changes nothing.
    let extended = fixture
        .store
        .extend_source_hold(
            &binding,
            &hold.hold_id,
            through,
            admission(expected_refs.len(), expected_bytes),
        )
        .unwrap();
    assert_extended_hold_matches_ledger(&fixture, &extended, through);
    assert_eq!(extended.expires_at, hold.expires_at);
    for replay_extension in [false, true] {
        let mut snapshot_hooks = 0;
        let status = fixture.store.source_hold_status_with_hook_for_test(
            &binding,
            &hold.hold_id,
            hold.captured_at,
            |phase| {
                if phase != kernel::SourceHoldCheckPhase::AfterSnapshot {
                    return;
                }
                snapshot_hooks += 1;
                if replay_extension {
                    let replayed = fixture
                        .store
                        .extend_source_hold(&binding, &hold.hold_id, through, wide_admission())
                        .unwrap();
                    assert_eq!(
                        replayed, extended,
                        "replay duplicates neither references nor charges"
                    );
                } else {
                    let receipt = fixture
                        .store
                        .commit(intent("unrelated-during-verification"), |_| {
                            Ok(String::new())
                        })
                        .unwrap();
                    assert!(!receipt.replayed);
                    assert!(receipt.commit_seq > through);
                }
            },
        );
        assert_eq!(snapshot_hooks, 1);
        assert_eq!(
            status,
            Ok(extended.clone()),
            "unchanged references need no retry: replay_extension={replay_extension}"
        );
    }
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM capture_pin_refs"),
        refs_before + i64::try_from(added).unwrap()
    );
    // Extending to S itself is an admitted no-op.
    assert_eq!(
        fixture
            .store
            .extend_source_hold(&binding, &hold.hold_id, hold.snapshot, wide_admission())
            .unwrap(),
        extended
    );
    // Paging still reads S; the window's descriptors are protected, not listed.
    assert_hold_matches_ledger_inventory(&fixture, &extended);

    // Acknowledgement now moves exactly to `through`, and replaying it is a
    // no-op after an uncertain response.
    fixture
        .store
        .acknowledge_through_source_hold(&binding, &hold.hold_id, through, 2)
        .unwrap();
    assert_eq!(fixture.checkpoint(), through);
    fixture
        .store
        .acknowledge_through_source_hold(&binding, &hold.hold_id, through, 3)
        .unwrap();
    assert_eq!(fixture.checkpoint(), through);
    assert_eq!(
        fixture
            .store
            .acknowledge_through_source_hold(&binding, &hold.hold_id, hold.snapshot, 3),
        Err(SourceHoldError::Kernel(
            kernel::KernelError::InvalidCheckpoint
        )),
        "a checkpoint never moves backward"
    );
    assert_eq!(fixture.checkpoint(), through);
    assert_eq!(
        fixture
            .store
            .source_hold_status(&binding, &hold.hold_id, hold.captured_at),
        Ok(extended.clone())
    );

    // A dead hold can neither extend nor acknowledge: expired, purge-degraded,
    // or released.
    fixture.publish("messages", "afterwards", 1, "afterwards");
    let later = fixture.store.tip().unwrap();
    let expiring = fixture
        .store
        .capture_source_hold(&binding, bounds(1_000))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1_001));
    assert!(wall_ms() >= expiring.expires_at);
    let expired = SourceHoldError::Invalid(SourceHoldInvalidity::Expired);
    assert_eq!(
        fixture
            .store
            .extend_source_hold(&binding, &expiring.hold_id, later, wide_admission())
            .unwrap_err(),
        expired
    );
    assert_eq!(
        fixture
            .store
            .acknowledge_through_source_hold(&binding, &expiring.hold_id, later, 9)
            .unwrap_err(),
        expired
    );
    let degraded_hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    let purged = fixture.held_all(&degraded_hold, 64)[0].evidence_id.clone();
    fixture.delete_evidence(&purged, ArtifactDeletionKind::Purge, 42);
    let degraded = SourceHoldError::Invalid(SourceHoldInvalidity::PurgeDegraded);
    let after_purge = fixture.store.tip().unwrap();
    assert_eq!(
        fixture
            .store
            .extend_source_hold(
                &binding,
                &degraded_hold.hold_id,
                after_purge,
                wide_admission()
            )
            .unwrap_err(),
        degraded
    );
    assert_eq!(
        fixture
            .store
            .acknowledge_through_source_hold(&binding, &degraded_hold.hold_id, after_purge, 9)
            .unwrap_err(),
        degraded
    );
    fixture
        .store
        .release_source_hold(&binding, &hold.hold_id, 9)
        .unwrap();
    let released = SourceHoldError::Invalid(SourceHoldInvalidity::Released);
    assert_eq!(
        fixture
            .store
            .extend_source_hold(&binding, &hold.hold_id, later, wide_admission())
            .unwrap_err(),
        released
    );
    assert_eq!(
        fixture
            .store
            .acknowledge_through_source_hold(&binding, &hold.hold_id, later, 9)
            .unwrap_err(),
        released
    );
    assert_eq!(
        fixture.checkpoint(),
        through,
        "no failure advances acknowledgement"
    );
}

#[test]
fn acknowledgement_authorization_outlives_the_checkpoint_commit() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let checkpoint = fixture.store.tip().unwrap();
    fixture
        .store
        .acknowledge_outbox(CONSUMER, checkpoint, 1)
        .unwrap();
    let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    fixture.publish("messages", "authorization", 1, "authorization window");
    let through = fixture.store.tip().unwrap();
    fixture
        .store
        .extend_source_hold(&binding, &hold.hold_id, through, wide_admission())
        .unwrap();
    let dropped = AtomicBool::new(false);

    let acknowledged = fixture
        .store
        .acknowledge_through_source_hold_if(&binding, &hold.hold_id, through, 2, || {
            Some(CheckpointAuthorization {
                fixture: &fixture,
                through,
                dropped: &dropped,
            })
        })
        .unwrap();

    assert!(acknowledged);
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn extension_after_window_deletion_distinguishes_purge_from_logical_delete() {
    for kind in [ArtifactDeletionKind::Delete, ArtifactDeletionKind::Purge] {
        let mut fixture = Fixture::open();
        let binding = fixture.binding();
        let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
        assert_eq!((hold.references, hold.encoded_bytes), (0, 0));

        fixture.publish("messages", "late", 1, "late evidence");
        let evidence = fixture.live_entry("messages", "late").clone();
        let through = fixture.store.tip().unwrap();
        assert!(through > hold.snapshot);
        let deleted = fixture.delete_evidence(&evidence.evidence_id, kind, wall_ms());
        assert!(deleted > through);
        assert_eq!(
            fixture
                .store
                .acknowledge_through_source_hold(&binding, &hold.hold_id, through, 1),
            Err(SourceHoldError::ExtensionIncomplete { uncovered: 1 })
        );
        assert_eq!(fixture.checkpoint(), 0);
        assert_eq!(
            fixture
                .store
                .source_hold_status(&binding, &hold.hold_id, wall_ms()),
            Ok(hold.clone())
        );

        let extended =
            fixture
                .store
                .extend_source_hold(&binding, &hold.hold_id, through, wide_admission());
        if kind == ArtifactDeletionKind::Purge {
            assert!(!fixture.object_present(&evidence.digest));
            assert_eq!(
                fixture.count("SELECT COUNT(*) FROM artifact_pending_unlinks"),
                0
            );
            assert_eq!(
                fixture.count("SELECT COUNT(*) FROM artifact_purge_tombstones"),
                1
            );
            assert!(
                extended.is_err(),
                "purged evidence must be refused before reference insertion: {extended:?}"
            );
            assert_eq!(extended, Err(SourceHoldError::PurgedEvidence));
            assert_eq!(fixture.count("SELECT COUNT(*) FROM capture_pin_refs"), 0);
            assert_eq!(
                fixture
                    .store
                    .acknowledge_through_source_hold(&binding, &hold.hold_id, through, 1),
                Err(SourceHoldError::ExtensionIncomplete { uncovered: 1 })
            );
            assert_eq!(fixture.checkpoint(), 0);
            assert_eq!(
                fixture
                    .store
                    .source_hold_status(&binding, &hold.hold_id, wall_ms()),
                Ok(hold.clone())
            );
            fixture
                .store
                .acknowledge_through_source_hold(&binding, &hold.hold_id, hold.snapshot, 1)
                .unwrap();
            assert_eq!(fixture.checkpoint(), hold.snapshot);
            assert_eq!(
                fixture.store.extend_source_hold(
                    &binding,
                    &hold.hold_id,
                    deleted,
                    wide_admission()
                ),
                Ok(hold.clone())
            );
            fixture
                .store
                .acknowledge_through_source_hold(&binding, &hold.hold_id, deleted, 1)
                .unwrap();
            assert_eq!(fixture.checkpoint(), deleted);
        } else {
            let extended = extended.unwrap();
            assert_extended_hold_matches_ledger(&fixture, &extended, through);
            assert_eq!(extended.references, 1);
            fixture
                .store
                .acknowledge_through_source_hold(&binding, &hold.hold_id, through, 1)
                .unwrap();
            assert_eq!(fixture.checkpoint(), through);
            assert_eq!(
                fixture
                    .store
                    .source_hold_status(&binding, &hold.hold_id, wall_ms()),
                Ok(extended)
            );
            assert_eq!(
                fs::read(fixture.object_path(&evidence.digest)).unwrap(),
                evidence.text.as_bytes()
            );
        }
    }
}

#[test]
fn an_empty_consumer_set_names_no_safe_horizon_so_replay_evidence_is_kept() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    fixture.publish("messages", "orphaned", 1, "kept for whoever registers next");
    let doomed = fixture
        .live_entry("messages", "orphaned")
        .evidence_id
        .clone();
    fixture.delete_evidence(&doomed, ArtifactDeletionKind::Delete, wall_ms());
    // The only consumer catches up and leaves; nothing bounds replay anymore.
    let tip = fixture.store.tip().unwrap();
    fixture.store.acknowledge_outbox(CONSUMER, tip, 1).unwrap();
    fixture
        .store
        .commit(intent("leave"), |envelope| {
            envelope.deregister_outbox_consumer(CONSUMER, 2)?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(fixture.count("SELECT COUNT(*) FROM outbox_consumers"), 0);
    let digest = fixture.entry_by_evidence(&doomed).digest.clone();
    // Inside the invalidation grace period the horizon is not the only keeper,
    // so the sweep reports nothing withheld for replay.
    let swept = fixture.store.run_staging_maintenance(wall_ms()).unwrap();
    assert_eq!(swept.artifact_gc.reclaimed_objects, 0);
    assert_eq!(
        swept.artifact_gc.withheld_for_replay, 0,
        "a candidate the grace period still keeps is not counted against the horizon"
    );
    assert!(fixture.object_present(&digest));
    let far = wall_ms() + i64::try_from(15 * DAY_MS).unwrap();
    // The sweep reports what the horizon kept: the one deleted artifact. The
    // ten live ones are kept by their live reference, not by the horizon.
    let swept = fixture.store.run_staging_maintenance(far).unwrap();
    assert_eq!(swept.artifact_gc.reclaimed_objects, 0);
    assert_eq!(
        swept.artifact_gc.withheld_for_replay, 1,
        "the no-consumer horizon reports the artifact it withholds"
    );
    assert!(
        fixture.object_present(&digest),
        "deleted descriptor evidence is kept while no consumer names a horizon"
    );
    // A consumer that registers starts below the oldest outbox row, so it
    // names a horizon behind the creation and still protects the bytes; once
    // it acknowledges past the creation the sweep proceeds.
    fixture
        .store
        .commit(intent("rejoin"), |envelope| {
            envelope.register_outbox_consumer(CONSUMER, 3)?;
            Ok(String::new())
        })
        .unwrap();
    assert!(fixture.checkpoint() < fixture.entry_by_evidence(&doomed).created);
    let swept = fixture.store.run_staging_maintenance(far).unwrap();
    assert_eq!(swept.artifact_gc.withheld_for_replay, 1);
    assert!(fixture.object_present(&digest));
    fixture
        .store
        .acknowledge_outbox(CONSUMER, fixture.store.tip().unwrap(), 3)
        .unwrap();
    let swept = fixture.store.run_staging_maintenance(far).unwrap();
    assert_eq!(
        swept.artifact_gc.withheld_for_replay, 0,
        "an acknowledged creation is no longer withheld"
    );
    assert_eq!(swept.artifact_gc.reclaimed_objects, 1);
    assert!(!fixture.object_present(&digest));
}

#[test]
fn retention_is_capped_and_a_removed_consumer_leaves_no_hold_behind() {
    for abandon in [false, true] {
        let mut fixture = Fixture::open();
        fixture.seed_five_classes();
        let binding = fixture.binding();

        // A lifetime past the kernel ceiling is refused whole; the ceiling itself
        // is admitted.
        assert_eq!(
            fixture
                .store
                .capture_source_hold(&binding, bounds(MAX_SOURCE_HOLD_LIFETIME_MS + 1))
                .unwrap_err(),
            SourceHoldError::InvalidRequest
        );
        let capped = fixture
            .store
            .capture_source_hold(&binding, bounds(MAX_SOURCE_HOLD_LIFETIME_MS))
            .unwrap();
        assert_eq!(
            capped.expires_at - capped.captured_at,
            i64::try_from(MAX_SOURCE_HOLD_LIFETIME_MS).unwrap()
        );

        // The consumer holds under two policy versions; another consumer holds too.
        let mut other_policy = binding.clone();
        other_policy.source_policy_version = "source-policy.v2".to_string();
        let second = fixture
            .store
            .capture_source_hold(&other_policy, wide())
            .unwrap();
        fixture
            .store
            .commit(intent("other-consumer"), |envelope| {
                envelope.register_outbox_consumer("other", 1)?;
                Ok(String::new())
            })
            .unwrap();
        let other_binding = SourceHoldBinding {
            consumer_id: "other".to_string(),
            ..binding.clone()
        };
        let other = fixture
            .store
            .capture_source_hold(&other_binding, wide())
            .unwrap();

        let tip = fixture.store.tip().unwrap();
        fixture.store.acknowledge_outbox(CONSUMER, tip, 1).unwrap();
        let release_started = wall_ms();
        fixture
            .store
            .commit(intent("remove-consumer"), |envelope| {
                if abandon {
                    envelope.abandon_outbox_consumer(
                        CONSUMER,
                        kernel::ConsumerAbandonment {
                            operator_id: "operator".to_string(),
                            reason: "retired".to_string(),
                            abandoned_at: 9,
                            barrier_id: None,
                        },
                    )?;
                } else {
                    envelope.deregister_outbox_consumer(CONSUMER, 9)?;
                }
                Ok(String::new())
            })
            .unwrap();
        let release_finished = wall_ms();
        for hold in [&capped, &second] {
            assert_eq!(
                fixture
                    .store
                    .source_hold_status(&hold.binding, &hold.hold_id, hold.captured_at),
                Err(SourceHoldError::Invalid(SourceHoldInvalidity::Released)),
                "abandon={abandon}, hold={}",
                hold.hold_id
            );
            assert!(fixture.pin_refs(&hold.hold_id).is_empty());
        }
        assert_eq!(
            fixture.count(
                "SELECT COUNT(*) FROM capture_pins
             WHERE pin_kind='source_hold' AND released_at IS NULL"
            ),
            1
        );
        assert_eq!(
            fixture
                .inspect()
                .query_row(
                    "SELECT COUNT(*) FROM capture_pins
                 WHERE pin_kind='source_hold' AND released_at BETWEEN ?1 AND ?2",
                    rusqlite::params![release_started, release_finished],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            fixture
                .store
                .source_hold_status(&other_binding, &other.hold_id, other.captured_at),
            Ok(other.clone())
        );
    }
}

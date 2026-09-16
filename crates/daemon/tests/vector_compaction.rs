//! Compaction against independent maps: the compacted base holds exactly the rows the prefix resolves to, ranks to the same scores, carries every tail change once, and refuses to publish a second history.

mod support;

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use daemon::projection_gates::Denial;
use daemon::vector_admission::{
    Census, DELTA_LIMIT, DISK_LIMIT, RESIDENT_LIMIT, Refusal, ResourceClass,
};
use daemon::vector_compaction::{Compacted, CompactionRefusal, Cut, compact, publish};
use daemon::vector_composition::{
    CompositionRefusal, Progress, Reconciled, SelectorState, reconcile, recover,
};
use daemon::vector_generation::{
    CODES_FILE, ExpectedVectors, ROWS_FILE, SIDECAR_FILE, VectorRefusal, footprint,
};
use host_runtime::generation::{GenerationError, ProfileEvent};
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::export::{ExportedRow, LiveRows};
use retrieval::dense::{Completion, RowAccess};
use support::dense_projection::{occurrence_id, reference};
use support::flock::try_exclusive;
use support::vector_reads::*;
use support::vector_store::{Fixture, unit};

fn max_entries() -> NonZeroUsize {
    NonZeroUsize::new(64).unwrap()
}

fn max_deltas() -> NonZeroUsize {
    NonZeroUsize::new(4).unwrap()
}

fn compact_view(
    fixture: &Fixture,
    view: &daemon::vector_reader::PinnedVectors,
) -> Result<Compacted, CompactionRefusal> {
    compact(
        view,
        &fixture.expected(),
        &fixture.staging(),
        max_entries(),
        &fixture.work_dir(),
    )
}

fn publish_compacted(
    fixture: &Fixture,
    compacted: &Compacted,
) -> Result<daemon::vector_compaction::Published, CompactionRefusal> {
    publish(
        compacted,
        &fixture.staging(),
        &fixture.expected(),
        max_deltas(),
        &fixture.work_dir(),
        &mut |_| Ok(()),
    )
}

/// One hand-written layer: its `(object, vector)` rows and the objects it tombstones.
type LayerMap<'a> = (&'a [(&'a str, Vec<f32>)], &'a [&'a str]);

/// The rows a set of layers resolves to by hand: the last layer that lists or tombstones an object decides.
fn expected_rows(layers: &[LayerMap<'_>]) -> Vec<(String, Vec<f32>)> {
    let mut winners: std::collections::BTreeMap<String, Option<Vec<f32>>> = Default::default();
    for (rows, tombstones) in layers {
        for (object, vector) in rows.iter() {
            winners.insert(occurrence_id(object), Some(vector.clone()));
        }
        for object in tombstones.iter() {
            winners.insert(occurrence_id(object), None);
        }
    }
    winners
        .into_iter()
        .filter_map(|(id, vector)| vector.map(|vector| (id, vector)))
        .collect()
}

#[test]
fn compaction_keeps_every_effective_row_applies_every_tombstone_and_ranks_to_the_same_scores() {
    let mut fixture = Fixture::new();
    let admitted: Vec<&str> = OBJECTS.iter().copied().filter(|o| *o != "gamma").collect();
    let projection = projection(&fixture, &admitted);
    let corpus = corpus();
    // A negative zero survives only a bit-preserving copy.
    let mut low = unit([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    low[0] = -0.0;
    let restored = unit([0.6, 0.0, 0.8, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let newer_gamma = unit([0.95, 0.05, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let first: &[(&str, Vec<f32>)] = &[("alpha", low.clone())];
    let second: &[(&str, Vec<f32>)] = &[("beta", restored.clone()), ("gamma", newer_gamma.clone())];
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    let d1 = fixture.layer_from(&export(first, &["beta", "delta"], 12));
    let d2 = fixture.layer_from(&export(second, &[], 14));
    let old_digest = fixture
        .publish(&fixture.compose(1, &base, &[d1, d2]).unwrap())
        .unwrap();
    let old_view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let query = axis(0);
    let before = rank_view(&fixture, &projection, &old_view, &query, 8).unwrap();

    let compacted = compact_view(&fixture, &old_view).unwrap();
    let expected = expected_rows(&[(&corpus, &[]), (first, &["beta", "delta"]), (second, &[])]);
    assert_eq!(compacted.winners, expected.len());
    // Two base rows superseded (`alpha`, `gamma`), one base row masked (`delta`); `beta`'s base row is superseded by the second delta's row over the first delta's tombstone.
    assert_eq!((compacted.superseded, compacted.masked), (3, 1));
    assert_eq!(
        compacted.cut,
        Cut {
            digest: old_digest.clone(),
            base: base.digest.clone(),
            deltas: vec![old_view.members()[1].clone(), old_view.members()[2].clone()],
        }
    );
    let published = publish_compacted(&fixture, &compacted).unwrap();
    assert_eq!(published.tail, Vec::<String>::new());
    assert_eq!(published.sequence, 2);
    let view_census = fixture.ledger.census();
    compacted.discard().unwrap();
    assert_eq!(
        fixture.ledger.census(),
        Census {
            held: view_census
                .held
                .iter()
                .filter(|(class, _)| **class == ResourceClass::LayerTables)
                .map(|(class, bytes)| (*class, *bytes))
                .collect(),
            resident: view_census.held[&ResourceClass::LayerTables],
            disk: 0,
            pinned: view_census.pinned,
        },
        "the compaction's reservations end with its output; the view's stay"
    );

    let new_view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    assert_eq!(new_view.digest(), published.digest);
    assert_eq!(new_view.members(), vec![published.base.clone()]);
    let compacted_base = &new_view.layers()[0];
    assert_eq!(
        compacted_base.occurrence_ids(),
        expected
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        compacted_base.tombstones().len(),
        0,
        "a base carries no tombstones; every prefix tombstone is applied by absence"
    );
    for (index, (_, vector)) in expected.iter().enumerate() {
        let bits: Vec<u32> = compacted_base
            .row(index)
            .unwrap()
            .iter()
            .map(|value| value.to_bits())
            .collect();
        assert_eq!(
            bits,
            vector
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            "row bits are the winners' exactly"
        );
    }
    assert_eq!(
        compacted_base.sidecar.checkpoint_commit_seq, 14,
        "the base stands at the prefix's last checkpoint"
    );

    let after = rank_view(&fixture, &projection, &new_view, &query, 8).unwrap();
    assert_eq!(keyed(&after), keyed(&before), "same winners, same scores");
    let mut live = expected.clone();
    live.retain(|(id, _)| *id != occurrence_id("gamma"));
    assert_eq!(keyed(&after), reference(&query, &live));
    // The kernel hides `gamma`; the row compaction kept for it is the newer one, and no older row of it is revived anywhere.
    let gamma_row = compacted_base
        .occurrence_ids()
        .iter()
        .position(|id| *id == occurrence_id("gamma"))
        .unwrap();
    assert_eq!(compacted_base.row(gamma_row).unwrap(), newer_gamma);
    assert_eq!(
        after.ranking.consumed.excluded,
        before.ranking.consumed.excluded
    );
    assert_eq!(after.ranking.completion, before.ranking.completion);
    assert_eq!(
        after.ranking.completion,
        Completion::Incomplete(retrieval::dense::IncompleteReason::DenseCoverageShortfall),
        "`delta` stays a coverage shortfall"
    );

    // The old view still reads its frozen prefix, and its members are retained until it lets go.
    assert_eq!(
        keyed(&rank_view(&fixture, &projection, &old_view, &query, 8).unwrap()),
        keyed(&before)
    );
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_generations, 0);
    assert!(report.retained_pinned >= 1);
    drop(old_view);
    let mut removed = 0;
    for _ in 0..3 {
        removed += fixture
            .store
            .prune(&BTreeSet::new())
            .unwrap()
            .removed_generations;
    }
    assert_eq!(
        removed, 4,
        "the old record, its base, and both deltas are reclaimed"
    );
    assert_eq!(
        fixture.generations(),
        BTreeSet::from([published.digest.clone(), published.base.clone()])
    );
}

#[test]
fn a_tail_published_after_the_cut_is_carried_once_with_its_precedence_and_a_stale_cut_cannot_republish()
 {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let corpus = corpus();
    let first: &[(&str, Vec<f32>)] = &[("alpha", axis(7))];
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    let d1 = fixture.layer_from(&export(first, &[], 12));
    fixture
        .publish(&fixture.compose(1, &base, &[d1]).unwrap())
        .unwrap();
    let cut_view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let compacted = compact_view(&fixture, &cut_view).unwrap();

    // Independently of the compactor, a later delta inserts nothing new but updates `epsilon`, deletes `beta`, and re-adds `alpha` over the first delta's row.
    let tail_rows: &[(&str, Vec<f32>)] = &[("epsilon", axis(3)), ("alpha", axis(1))];
    let d2 = fixture.layer_from(&export(tail_rows, &["beta"], 16));
    let d1_verified = daemon::vector_generation::verify(
        &fixture.store,
        &cut_view.members()[1],
        &fixture.expected(),
    )
    .unwrap();
    let with_tail = fixture.compose(2, &base, &[d1_verified, d2]).unwrap();
    fixture.publish(&with_tail).unwrap();
    let full_view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let query = axis(0);
    let full = rank_view(&fixture, &projection, &full_view, &query, 8).unwrap();
    let expected = expected_rows(&[(&corpus, &[]), (first, &[]), (tail_rows, &["beta"])]);
    assert_eq!(keyed(&full), reference(&query, &expected));

    // While the compactor reads back and renames, a competing mutator cannot take the transaction lock.
    let lock = transaction_lock(&fixture);
    let mut renames = 0;
    let published = publish(
        &compacted,
        &fixture.staging(),
        &fixture.expected(),
        max_deltas(),
        &fixture.work_dir(),
        &mut |event| {
            if event == ProfileEvent::BeforeRename {
                renames += 1;
                assert!(
                    !try_exclusive(&lock),
                    "the exclusive lock is held through the rename"
                );
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(renames, 1);
    assert_eq!(
        published.tail,
        vec![with_tail.deltas[1].clone()],
        "the tail is exactly what followed the cut"
    );
    assert_eq!(published.sequence, 3);
    let compacted_view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    assert_eq!(
        compacted_view.members(),
        vec![published.base.clone(), with_tail.deltas[1].clone()]
    );
    let after = rank_view(&fixture, &projection, &compacted_view, &query, 8).unwrap();
    assert_eq!(
        keyed(&after),
        keyed(&full),
        "the tail's update, delete, and re-add stand above the compacted base exactly as before"
    );
    assert_eq!(after.layers.winners, full.layers.winners);
    assert_eq!(
        after.layers.masked, 1,
        "`beta`'s compacted row is masked by the tail once"
    );
    assert_eq!(
        after.layers.superseded, 2,
        "`epsilon` and `alpha` are superseded once each"
    );

    // A compaction of a cut whose base already moved refuses before staging: the full view's prefix compacts to a base nobody has staged, and none reaches the store.
    let stale = compact_view(&fixture, &full_view).unwrap();
    let generations = fixture.generations();
    assert!(!generations.contains(&stale.built.digest()));
    assert_eq!(
        publish_compacted(&fixture, &stale).unwrap_err(),
        CompactionRefusal::PrefixMoved
    );
    assert_eq!(
        fixture.generations(),
        generations,
        "a refused prefix stages nothing"
    );
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        host_runtime::generation::CurrentProfile::Current(published.digest.clone())
    );
    compacted.discard().unwrap();
    stale.discard().unwrap();
    assert_eq!(held(&fixture.ledger, ResourceClass::CompactionScratch), 0);
}

#[test]
fn an_unknown_publication_outcome_is_reconciled_and_never_republished_as_a_second_history() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    let d1 = fixture.layer_from(&export(&[("alpha", axis(7))], &[], 12));
    fixture
        .publish(&fixture.compose(1, &base, &[d1]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let compacted = compact_view(&fixture, &view).unwrap();
    let outcome = publish(
        &compacted,
        &fixture.staging(),
        &fixture.expected(),
        max_deltas(),
        &fixture.work_dir(),
        &mut |event| {
            if event == ProfileEvent::AfterRename {
                Err(GenerationError::NativePayloadInvalid { detail: "cut" })
            } else {
                Ok(())
            }
        },
    );
    let Err(CompactionRefusal::Unknown { digest, progress }) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(progress, Progress::Acknowledged);
    assert_eq!(
        reconcile(&fixture.store, fixture.transaction(), &digest).unwrap(),
        Reconciled::Published
    );
    let recovered = recover(
        &fixture.store,
        fixture.transaction(),
        &fixture.expected(),
        max_deltas(),
        NonZeroUsize::new(8).unwrap(),
    )
    .unwrap();
    assert_eq!(recovered.composition.digest, digest);
    assert_eq!(recovered.selector, SelectorState::Current);
    assert_eq!(recovered.composition.deltas.len(), 0);
    // A blind retry from the same cut finds the selection standing on the compacted base and refuses; the compacted base itself is staged idempotently.
    let retry = compact_view(&fixture, &view).unwrap();
    assert_eq!(
        retry.built.digest(),
        compacted.built.digest(),
        "the same prefix compacts to the same base"
    );
    assert_eq!(
        publish_compacted(&fixture, &retry).unwrap_err(),
        CompactionRefusal::PrefixMoved
    );
    compacted.discard().unwrap();
    retry.discard().unwrap();

    // A rename that never happened leaves the old selection; reconciliation says so, and the retry from the same cut, through the same record directory, publishes once, with the base staged a second time charged nothing.
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    let d1 = fixture.layer_from(&export(&[("alpha", axis(7))], &[], 12));
    let old = fixture
        .publish(&fixture.compose(1, &base, &[d1]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let compacted = compact_view(&fixture, &view).unwrap();
    let before_attempts = fixture.generations();
    assert!(!before_attempts.contains(&compacted.built.digest()));
    let record_dir = fixture.work_dir();
    let outcome = publish(
        &compacted,
        &fixture.staging(),
        &fixture.expected(),
        max_deltas(),
        &record_dir,
        &mut |event| {
            if event == ProfileEvent::BeforeRename {
                Err(GenerationError::NativePayloadInvalid { detail: "cut" })
            } else {
                Ok(())
            }
        },
    );
    assert!(
        matches!(outcome, Err(CompactionRefusal::Composition(_))),
        "a failure before the rename is a known refusal, not an unknown outcome: {outcome:?}"
    );
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        host_runtime::generation::CurrentProfile::Current(old.clone())
    );
    let generations = fixture.generations();
    assert!(
        generations.contains(&compacted.built.digest()),
        "the first attempt staged the new base before the rename failed"
    );
    assert_eq!(generations.len(), before_attempts.len() + 2);
    assert!(
        std::fs::read_dir(&record_dir).unwrap().next().is_none(),
        "the record files are removed once staged, so the directory serves the retry"
    );
    let published = publish(
        &compacted,
        &fixture.staging(),
        &fixture.expected(),
        max_deltas(),
        &record_dir,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(published.sequence, 2);
    assert_eq!(
        fixture.generations(),
        generations,
        "the base and the record were staged by the first attempt; the retry adds nothing"
    );
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        host_runtime::generation::CurrentProfile::Current(published.digest)
    );
    assert!(std::fs::read_dir(&record_dir).unwrap().next().is_none());
    compacted.discard().unwrap();
}

#[test]
fn reservations_are_taken_before_any_read_released_with_the_output_and_a_short_limit_writes_nothing()
 {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let resident_before = fixture.ledger.census().resident;

    let compacted = compact_view(&fixture, &view).unwrap();
    let census = fixture.ledger.census();
    assert_eq!(
        held(&fixture.ledger, ResourceClass::RowBuffers),
        0,
        "the rows are on disk once the build returns"
    );
    assert_eq!(census.resident, resident_before);
    assert_eq!(
        held(&fixture.ledger, ResourceClass::CompactionScratch),
        inventory_bytes(&compacted),
        "the scratch reservation is exactly what the build wrote"
    );
    let work_dir = compacted.built.dir.clone();
    assert!(std::fs::read_dir(&work_dir).unwrap().next().is_some());
    compacted.discard().unwrap();
    assert!(!work_dir.exists(), "discarding removes the files");
    assert_eq!(fixture.ledger.census().disk, 0);

    // One byte short of the reservation refuses before any row is read; the reservation is at least the build's own footprint.
    fixture.set_limit(RESIDENT_LIMIT, resident_before);
    let dir = fixture.work_dir();
    let refusal = compact(
        &view,
        &fixture.expected(),
        &fixture.staging(),
        max_entries(),
        &dir,
    )
    .unwrap_err();
    let CompactionRefusal::Reservation(Refusal::Denied(Denial::LimitExceeded {
        limit,
        observed,
        max,
    })) = refusal.clone()
    else {
        panic!("{refusal:?}");
    };
    assert_eq!((limit.as_str(), max), (RESIDENT_LIMIT, resident_before));
    let build_footprint = footprint(
        &fixture.expected(),
        &view.layers()[0].checkpoint,
        view.layers()[0].occurrence_ids().iter().map(String::as_str),
        std::iter::empty(),
    );
    assert!(observed - resident_before >= build_footprint.resident);
    assert!(observed - resident_before < 2 * build_footprint.resident);
    fixture.set_limit(RESIDENT_LIMIT, observed - 1);
    let refusal = compact(
        &view,
        &fixture.expected(),
        &fixture.staging(),
        max_entries(),
        &dir,
    )
    .unwrap_err();
    assert_eq!(
        refusal,
        CompactionRefusal::Reservation(Refusal::Denied(Denial::LimitExceeded {
            limit: RESIDENT_LIMIT.to_owned(),
            observed,
            max: observed - 1
        }))
    );
    assert!(
        std::fs::read_dir(&dir).unwrap().next().is_none(),
        "a refused compaction writes nothing"
    );
    fixture.set_limit(RESIDENT_LIMIT, u64::MAX);
    fixture.set_limit(DISK_LIMIT, 1);
    let dir = fixture.work_dir();
    let refusal = compact(
        &view,
        &fixture.expected(),
        &fixture.staging(),
        max_entries(),
        &dir,
    )
    .unwrap_err();
    assert!(
        matches!(&refusal, CompactionRefusal::Reservation(Refusal::Denied(Denial::LimitExceeded { limit, .. })) if limit == DISK_LIMIT),
        "{refusal:?}"
    );
    assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
    assert_eq!(
        fixture.ledger.census().resident,
        resident_before,
        "a refused disk reservation releases the row reservation taken before it"
    );
}

#[test]
fn at_the_delta_cap_further_deltas_are_refused_and_compaction_clears_the_cap() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    let d1 = fixture.layer_from(&export(&[("alpha", axis(7))], &[], 12));
    let d2 = fixture.layer_from(&export(&[("beta", axis(6))], &[], 14));
    fixture.set_limit(DELTA_LIMIT, 1);
    let one = fixture.compose(1, &base, &[d1]).unwrap();
    fixture.publish(&one).unwrap();
    let d1_again =
        daemon::vector_generation::verify(&fixture.store, &one.deltas[0], &fixture.expected())
            .unwrap();
    let two = fixture.compose(2, &base, &[d1_again, d2]).unwrap();
    assert_eq!(
        fixture.publish(&two).unwrap_err(),
        CompositionRefusal::Deltas(Denial::LimitExceeded {
            limit: DELTA_LIMIT.to_owned(),
            observed: 2,
            max: 1
        })
    );

    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let compacted = compact_view(&fixture, &view).unwrap();
    let published = publish_compacted(&fixture, &compacted).unwrap();
    assert_eq!(published.tail.len(), 0, "the cap is clear again");
    let compacted_base =
        daemon::vector_generation::verify(&fixture.store, &published.base, &fixture.expected())
            .unwrap();
    let d2_again =
        daemon::vector_generation::verify(&fixture.store, &two.deltas[1], &fixture.expected())
            .unwrap();
    let next = fixture
        .compose(published.sequence + 1, &compacted_base, &[d2_again])
        .unwrap();
    assert!(
        fixture.publish(&next).is_ok(),
        "one delta over the compacted base admits"
    );
}

#[test]
fn a_fully_masked_base_compacts_to_the_deltas_rows_alone_and_a_foreign_expectation_refuses() {
    let mut fixture = Fixture::new();
    // A delta needs a row to be built, so the base is masked entirely by a delta carrying one row for an object outside the corpus; that row is the whole compacted base.
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    let masking: &[(&str, Vec<f32>)] = &[("zeta", axis(2))];
    let all_masked = fixture.layer_from(&export(masking, &OBJECTS, 12));
    fixture
        .publish(&fixture.compose(1, &base, &[all_masked]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let compacted = compact_view(&fixture, &view).unwrap();
    assert_eq!(
        (compacted.winners, compacted.masked),
        (1, 5),
        "every base row is masked; the delta's own row is the whole base"
    );
    compacted.discard().unwrap();

    // A view of another model space refuses before anything is reserved, read, or written; no mislabelled base can reach the store.
    let mut other = fixture.generation.clone();
    other.embedding_model = "another-model".to_owned();
    let foreign = ExpectedVectors {
        generation: &other,
        ..fixture.expected()
    };
    let before = fixture.generations();
    let dir = fixture.work_dir();
    let refusal = compact(&view, &foreign, &fixture.staging(), max_entries(), &dir).unwrap_err();
    assert_eq!(
        refusal,
        CompactionRefusal::Identity {
            digest: view.members()[0].clone(),
            field: "embedding_model"
        }
    );
    assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
    assert_eq!(fixture.generations(), before);
    assert_eq!(held(&fixture.ledger, ResourceClass::RowBuffers), 0);
    assert_eq!(held(&fixture.ledger, ResourceClass::CompactionScratch), 0);
}

fn inventory_bytes(compacted: &Compacted) -> u64 {
    compacted
        .built
        .sidecar
        .stage_manifest()
        .files
        .iter()
        .map(|file| file.size)
        .sum()
}

#[test]
fn the_scratch_reservation_is_sized_from_the_winners_identifiers_and_the_sidecar_it_writes() {
    // Each identifier is 1000 bytes, so the reservation must include identifier lengths.
    let mut fixture = Fixture::new();
    let long_ids: Vec<(String, Vec<f32>)> = (0..5u32)
        .map(|i| (format!("{i:02}").repeat(500), axis(i as usize)))
        .collect();
    let long_export = LiveRows {
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: 9,
            checkpoint_commit_seq: 10,
            hold_id: "hold-7".to_owned(),
        },
        rows: long_ids
            .iter()
            .map(|(occurrence_id, vector)| ExportedRow {
                occurrence_id: occurrence_id.clone(),
                vector: vector.clone(),
            })
            .collect(),
        tombstones: Vec::new(),
    };
    let base = fixture.layer_from(&long_export);
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let compacted = compact_view(&fixture, &view).unwrap();
    assert_eq!(
        held(&fixture.ledger, ResourceClass::CompactionScratch),
        inventory_bytes(&compacted),
        "the scratch reservation is exactly the files the build wrote"
    );
    compacted.discard().unwrap();

    // The model name is 6 KiB, so the sidecar alone exceeds a 4 KiB fixed allowance.
    let mut fixture = Fixture::new();
    fixture.generation.embedding_model = "model-".repeat(1024);
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let compacted = compact_view(&fixture, &view).unwrap();
    assert!(compacted.built.sidecar.canonical_bytes().len() > 6000);
    assert_eq!(
        held(&fixture.ledger, ResourceClass::CompactionScratch),
        inventory_bytes(&compacted),
        "the sidecar's own bytes are part of the reservation"
    );
    compacted.discard().unwrap();
}

#[test]
fn a_build_that_fails_after_writing_leaves_no_file_of_its_own_and_no_charge() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    // A directory at `CODES_FILE` makes the codes write fail after the rows write succeeded.
    let dir = fixture.work_dir();
    std::fs::create_dir(dir.join(CODES_FILE)).unwrap();
    let refusal = compact(
        &view,
        &fixture.expected(),
        &fixture.staging(),
        max_entries(),
        &dir,
    )
    .unwrap_err();
    assert!(
        matches!(refusal, CompactionRefusal::Build(VectorRefusal::Io(_))),
        "{refusal:?}"
    );
    assert!(
        !dir.join(ROWS_FILE).exists(),
        "the rows the build wrote before failing are removed"
    );
    assert!(
        dir.join(CODES_FILE).is_dir(),
        "what the build did not write is left alone"
    );
    assert_eq!(held(&fixture.ledger, ResourceClass::RowBuffers), 0);
    assert_eq!(held(&fixture.ledger, ResourceClass::CompactionScratch), 0);
}

#[test]
fn discard_removes_only_the_builds_files_and_drop_removes_them_too() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();

    // Discard unlinks the build's own files, leaves a foreign file in place, and reports the directory it cannot remove.
    let compacted = compact_view(&fixture, &view).unwrap();
    let dir = compacted.built.dir.clone();
    let foreign = dir.join("keep.txt");
    std::fs::write(&foreign, b"not the build's").unwrap();
    assert!(matches!(
        compacted.discard(),
        Err(CompactionRefusal::Discard(_))
    ));
    assert!(foreign.exists(), "discard deletes nothing it did not write");
    assert!(!dir.join(ROWS_FILE).exists());
    assert!(!dir.join(SIDECAR_FILE).exists());
    assert_eq!(held(&fixture.ledger, ResourceClass::CompactionScratch), 0);

    // Dropping `Compacted` removes the build's files and releases their charge.
    let compacted = compact_view(&fixture, &view).unwrap();
    let dir = compacted.built.dir.clone();
    assert!(dir.join(ROWS_FILE).exists());
    drop(compacted);
    assert!(!dir.exists(), "the work directory goes with the output");
    assert_eq!(held(&fixture.ledger, ResourceClass::CompactionScratch), 0);
}

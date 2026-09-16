//! Compaction against independent maps: the compacted base holds exactly the rows the prefix resolves to, ranks to the same scores, carries every tail change once, and refuses to publish a second history.

mod support;

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use daemon::projection_gates::Denial;
use daemon::vector_admission::{
    Census, DELTA_LIMIT, DISK_LIMIT, RESIDENT_LIMIT, Refusal, ResourceClass,
};
use daemon::vector_compaction::{Compacted, CompactionRefusal, Cut, compact, due, publish};
use daemon::vector_composition::{
    CompositionRefusal, Progress, Reconciled, SelectorState, reconcile, recover,
};
use daemon::vector_generation::ExpectedVectors;
use host_runtime::generation::{GenerationError, ProfileEvent};
use retrieval::dense::{Completion, RowAccess};
use support::dense_projection::{occurrence_id, reference};
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
        &fixture.ledger,
        &fixture.admission,
        &fixture.store,
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
    let low = unit([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
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
    drop(compacted);
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
    assert_eq!(new_view.digest, published.digest);
    assert_eq!(new_view.members(), vec![published.base.clone()]);
    let compacted_base = &new_view.layers[0];
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
        assert_eq!(
            &compacted_base.row(index).unwrap(),
            vector,
            "row bytes are the winners' exactly"
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
    // The kernel hides `gamma`; its newer row is the one compaction kept, and no older row of it is revived.
    assert!(
        !compacted_base
            .occurrence_ids()
            .contains(&occurrence_id("gamma"))
            || compacted_base
                .row(
                    compacted_base
                        .occurrence_ids()
                        .iter()
                        .position(|id| *id == occurrence_id("gamma"))
                        .unwrap()
                )
                .unwrap()
                == newer_gamma
    );
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

    let published = publish_compacted(&fixture, &compacted).unwrap();
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

    // A compaction of the cut that already moved refuses: the selection no longer stands on that base.
    let stale = compact_view(&fixture, &cut_view).unwrap();
    assert_eq!(
        publish_compacted(&fixture, &stale).unwrap_err(),
        CompactionRefusal::PrefixMoved
    );
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        host_runtime::generation::CurrentProfile::Current(published.digest.clone())
    );
    drop((compacted, stale));
    assert_eq!(held(&fixture.ledger, ResourceClass::RowBuffers), 0);
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
}

#[test]
fn reservations_precede_any_read_and_a_short_limit_refuses_with_nothing_written() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let resident_before = fixture.ledger.census().resident;

    let compacted = compact_view(&fixture, &view).unwrap();
    let census = fixture.ledger.census();
    assert_eq!(held(&fixture.ledger, ResourceClass::RowBuffers), 5 * 8 * 4);
    assert_eq!(census.resident, resident_before + 5 * 8 * 4);
    assert!(
        census.disk > 0,
        "the compacted files are scratch until staged"
    );
    let work_dir = compacted.built.dir.clone();
    drop(compacted);
    assert_eq!(fixture.ledger.census().resident, resident_before);
    assert_eq!(fixture.ledger.census().disk, 0);

    fixture.set_limit(RESIDENT_LIMIT, resident_before + 5 * 8 * 4 - 1);
    let dir = fixture.work_dir();
    let refusal = compact(
        &view,
        &fixture.expected(),
        &fixture.ledger,
        &fixture.admission,
        &fixture.store,
        max_entries(),
        &dir,
    )
    .unwrap_err();
    assert!(
        matches!(
            refusal,
            CompactionRefusal::Reservation(Refusal::Denied(Denial::LimitExceeded { .. }))
        ),
        "{refusal:?}"
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
        &fixture.ledger,
        &fixture.admission,
        &fixture.store,
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
    let _ = work_dir;
}

#[test]
fn at_the_delta_cap_further_deltas_are_refused_and_compaction_is_due_and_clears_the_cap() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    let d1 = fixture.layer_from(&export(&[("alpha", axis(7))], &[], 12));
    let d2 = fixture.layer_from(&export(&[("beta", axis(6))], &[], 14));
    fixture.set_limit(DELTA_LIMIT, 1);
    let one = fixture.compose(1, &base, &[d1]).unwrap();
    fixture.publish(&one).unwrap();
    assert!(due(one.deltas.len(), 1));
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
    assert_eq!(published.tail.len(), 0);
    assert!(!due(published.tail.len(), 1), "the cap is clear again");
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
    drop(compacted);

    let mut other = fixture.generation.clone();
    other.embedding_model = "another-model".to_owned();
    let foreign = ExpectedVectors {
        generation: &other,
        ..fixture.expected()
    };
    let mut fixture2 = Fixture::new();
    let base2 = fixture2.layer_from(&export(&corpus(), &[], 10));
    fixture2
        .publish(&fixture2.compose(1, &base2, &[]).unwrap())
        .unwrap();
    let view2 = acquire_view(&mut fixture2, &mut |_| {}).unwrap();
    let compacted = compact(
        &view2,
        &foreign,
        &fixture2.ledger,
        &fixture2.admission,
        &fixture2.store,
        max_entries(),
        &fixture2.work_dir(),
    )
    .unwrap();
    let refusal = publish(
        &compacted,
        &fixture2.staging(),
        &foreign,
        max_deltas(),
        &fixture2.work_dir(),
        &mut |_| Ok(()),
    )
    .unwrap_err();
    // The selected composition is read back under the expectation and does not carry it, so no base is composed at all.
    assert_eq!(
        refusal,
        CompactionRefusal::Composition(CompositionRefusal::NotComposition("identity"))
    );
    assert_eq!(
        fixture2.store.read_vector_current().unwrap(),
        host_runtime::generation::CurrentProfile::Current(view2.digest.clone()),
        "no fabricated composition is selected"
    );
}

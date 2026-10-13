mod support;

use std::sync::Arc;

use daemon::query_route::{CompressedLimits, QueryRouteLimits};
use daemon::vector_admission::ResourceClass;
use daemon::vector_current::CurrentVectors;
use daemon::vector_reader::{AcquireEvent, AcquireRefusal};
use retrieval::batch::ProjectionCheckpoint;
use support::vector_reads::*;
use support::vector_store::Fixture;

fn owner(fixture: &Fixture) -> CurrentVectors {
    CurrentVectors::new(
        Some(fixture.root.path().to_path_buf()),
        bounds(),
        Arc::clone(&fixture.ledger),
        fixture.admission.clone(),
    )
}

/// Runs `step` with the fixture's exclusive transaction released, as the reader requires, and counts the acquisitions it starts.
fn counted<T>(
    fixture: &mut Fixture,
    step: impl FnOnce(&Fixture, &mut dyn FnMut(AcquireEvent)) -> T,
) -> (T, usize) {
    fixture.release_transaction();
    let mut acquisitions = 0;
    let out = step(fixture, &mut |event| {
        if event == AcquireEvent::BeforeVerification {
            acquisitions += 1;
        }
    });
    fixture.reacquire_transaction();
    (out, acquisitions)
}

#[test]
fn the_owner_shares_one_view_until_the_selector_moves() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let owner = owner(&fixture);

    let ((first, second), acquisitions) = counted(&mut fixture, |fixture, observe| {
        let expected = fixture.expected();
        let first = owner.view(&expected, observe).unwrap();
        let second = owner.view(&expected, observe).unwrap();
        (first, second)
    });
    assert_eq!(
        acquisitions, 1,
        "the second caller shares the verified view"
    );
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.members()[0], base.digest);

    let (_, replacement) = publish_replacement(&fixture, &corpus(), 2);
    let (third, acquisitions) = counted(&mut fixture, |fixture, observe| {
        owner.view(&fixture.expected(), observe).unwrap()
    });
    assert_eq!(
        acquisitions, 1,
        "the moved selector forces a new acquisition"
    );
    assert!(!Arc::ptr_eq(&first, &third));
    assert_eq!(third.members()[0], replacement.digest);
    assert_eq!(first.members()[0], base.digest, "the old view stays whole");
    let (fourth, acquisitions) = counted(&mut fixture, |fixture, observe| {
        owner.view(&fixture.expected(), observe).unwrap()
    });
    assert_eq!(acquisitions, 0);
    assert!(Arc::ptr_eq(&third, &fourth));
}

#[test]
fn a_caller_expecting_another_identity_is_refused_and_the_owner_reacquires_for_the_next() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let owner = owner(&fixture);
    let (held_view, acquisitions) = counted(&mut fixture, |fixture, observe| {
        owner.view(&fixture.expected(), observe).unwrap()
    });
    assert_eq!(acquisitions, 1);

    let other = ProjectionCheckpoint {
        snapshot_commit_seq: 1,
        checkpoint_commit_seq: 2,
        hold_id: "another-hold".to_owned(),
    };
    let (refused, _) = counted(&mut fixture, |fixture, observe| {
        let expected = daemon::vector_generation::ExpectedVectors {
            checkpoint: Some(&other),
            ..fixture.expected()
        };
        owner.view(&expected, observe)
    });
    assert!(
        matches!(refused, Err(AcquireRefusal::Unavailable(_))),
        "{refused:?}"
    );

    let (again, acquisitions) = counted(&mut fixture, |fixture, observe| {
        owner.view(&fixture.expected(), observe).unwrap()
    });
    assert_eq!(acquisitions, 1, "the refused attempt left no view held");
    assert!(!Arc::ptr_eq(&held_view, &again));
}

#[test]
fn a_released_view_leaves_the_ledger_once_its_last_holder_ends() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let owner = owner(&fixture);
    let (view, _) = counted(&mut fixture, |fixture, observe| {
        owner.view(&fixture.expected(), observe).unwrap()
    });
    let tables = held(&fixture.ledger, ResourceClass::LayerTables);
    assert!(tables > 0);
    owner.release();
    assert_eq!(
        held(&fixture.ledger, ResourceClass::LayerTables),
        tables,
        "a request still holding the view keeps its charge"
    );
    drop(view);
    assert_eq!(held(&fixture.ledger, ResourceClass::LayerTables), 0);
    assert_eq!(fixture.ledger.census().pinned, 0);
}

#[test]
fn the_production_compressed_bounds_give_the_route_a_pool_of_256_for_its_64() {
    let route = QueryRouteLimits::production();
    let dense = route.dense.expect("production declares the dense lane");
    assert_eq!(dense.k.get(), 64);
    let capacity = CompressedLimits::production().capacity(&dense).unwrap();
    assert_eq!(capacity.candidates().get(), 256);
    assert_eq!(capacity.k().get(), 64);
}

#[test]
fn the_route_vectors_rank_through_the_shared_view() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let owner = owner(&fixture);
    let ((vectors, view), acquisitions) = counted(&mut fixture, |fixture, observe| {
        let expected = fixture.expected();
        let view = owner.view(&expected, observe).unwrap();
        let vectors = owner
            .dense_vectors(
                &expected,
                fixture.activation(),
                CompressedLimits::production(),
            )
            .unwrap();
        (vectors, view)
    });
    assert_eq!(acquisitions, 1, "the route vectors reuse the held view");
    assert!(Arc::ptr_eq(vectors.view(), &view));
}

#[test]
fn a_selector_unreadable_under_a_held_lifecycle_lock_keeps_the_verified_view() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let owner = owner(&fixture);
    let (held_view, _) = counted(&mut fixture, |fixture, observe| {
        owner.view(&fixture.expected(), observe).unwrap()
    });
    // The fixture holds the exclusive transaction here, so reading the selector outlasts the shared lock's bounded wait.
    assert!(matches!(
        held_view.selector_holds(Some(fixture.root.path())),
        Err(AcquireRefusal::Unprotected(_))
    ));
    let mut acquisitions = 0;
    let served = owner
        .view(&fixture.expected(), &mut |event| {
            if event == AcquireEvent::BeforeVerification {
                acquisitions += 1;
            }
        })
        .unwrap();
    assert_eq!(
        acquisitions, 0,
        "the verified view is served without a new hash"
    );
    assert!(Arc::ptr_eq(&held_view, &served));
}

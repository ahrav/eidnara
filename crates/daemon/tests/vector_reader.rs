//! The file-backed reader over a real lifecycle store and a real projection: pins witnessed by competing flocks, rows read by offset against an independent f64 reference, and ownership that outlives the caller.

mod support;

#[allow(dead_code)]
#[path = "support/alloc_recorder.rs"]
mod alloc_recorder;

#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use daemon::projection_gates::{Admission, Denial};
use daemon::vector_admission::{Ledger, Pool, RESIDENT_LIMIT, Refusal, ResourceClass};
use daemon::vector_composition::publish;
use daemon::vector_generation::{ExpectedVectors, ROWS_FILE, Staging, VerifiedVectors};
use daemon::vector_reader::{
    AcquireEvent, AcquireRefusal, PinnedVectors, RankRefusal, RankRequest, ReaderBounds, acquire,
    rank,
};
use host_runtime::generation::{CurrentProfile, GenerationStore, VECTOR_PROFILE_NAME};
use host_runtime::lifecycle::{
    LifecycleTransactionLock, TRANSACTION_LOCK_NAME, coordination_dir_path,
};
use kernel::EligibilityVerdict;
use kernel::applicability::EvalBudget;
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::export::{ExportedRow, LiveRows};
use retrieval::dense::scalar;
use retrieval::dense::{
    Completion, IncompleteReason, LayerAccount, LayeredRanking, LayeredRefusal, OracleBounds,
    OracleRefusal, RowAccess, RowFault,
};
use support::dense_projection::{Projection, occurrence_id, reference};
use support::flock::try_exclusive;
use support::vector_store::{Fixture, KERNEL, generation, unit};
use tokio_util::sync::CancellationToken;

const OBJECTS: [&str; 5] = ["alpha", "beta", "gamma", "delta", "epsilon"];

fn corpus() -> Vec<(&'static str, Vec<f32>)> {
    vec![
        ("alpha", unit([0.9, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1])),
        ("beta", unit([0.7, 0.3, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0])),
        ("gamma", unit([0.5, 0.5, 0.0, 0.3, 0.0, 0.0, 0.0, 0.0])),
        ("delta", unit([0.3, 0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.0])),
        ("epsilon", unit([0.1, 0.0, 0.0, 0.0, 0.0, 0.9, 0.0, 0.0])),
    ]
}

fn axis(index: usize) -> Vec<f32> {
    let mut raw = [0.0f32; 8];
    raw[index] = 1.0;
    unit(raw)
}

/// An export whose identifiers are the projection's, so the layer names what the walk visits.
fn export(rows: &[(&str, Vec<f32>)], tombstones: &[&str], checkpoint: i64) -> LiveRows {
    let mut rows: Vec<ExportedRow> = rows
        .iter()
        .map(|(object, vector)| ExportedRow {
            occurrence_id: occurrence_id(object),
            vector: vector.clone(),
        })
        .collect();
    rows.sort_by(|a, b| a.occurrence_id.cmp(&b.occurrence_id));
    let mut tombstones: Vec<String> = tombstones
        .iter()
        .map(|object| occurrence_id(object))
        .collect();
    tombstones.sort();
    LiveRows {
        generation: generation(),
        kernel_incarnation_id: KERNEL.to_owned(),
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: checkpoint - 1,
            checkpoint_commit_seq: checkpoint,
            hold_id: "hold-7".to_owned(),
        },
        rows,
        tombstones,
    }
}

fn bounds() -> ReaderBounds {
    ReaderBounds {
        max_deltas: NonZeroUsize::new(4).unwrap(),
        recovery_bound: NonZeroUsize::new(8).unwrap(),
    }
}

fn oracle_bounds(k: usize) -> OracleBounds {
    OracleBounds {
        k: NonZeroUsize::new(k).unwrap(),
        page_rows: NonZeroUsize::new(2).unwrap(),
        max_rows: NonZeroUsize::new(64).unwrap(),
    }
}

/// One page of two rows plus one raw row, eight coordinates wide.
const PAGE_SCRATCH: u64 = (2 + 1) * 8 * 4;

fn projection(fixture: &Fixture, admitted: &[&str]) -> Projection {
    Projection::new(
        fixture.root.path(),
        &fixture.identity,
        &fixture.generation,
        &OBJECTS,
        admitted,
    )
}

fn transaction_lock(fixture: &Fixture) -> PathBuf {
    coordination_dir_path(Some(fixture.root.path()))
        .unwrap()
        .join(TRANSACTION_LOCK_NAME)
}

/// Acquires under the reader's own shared protection, giving up the fixture's exclusive transaction for the duration.
fn acquire_view(
    fixture: &mut Fixture,
    observe: &mut dyn FnMut(AcquireEvent),
) -> Result<Arc<PinnedVectors>, AcquireRefusal> {
    fixture.release_transaction();
    let view = acquire(
        Some(fixture.root.path()),
        &fixture.expected(),
        bounds(),
        &fixture.ledger,
        &fixture.admission,
        observe,
    );
    fixture.reacquire_transaction();
    view
}

/// What the ledger holds for `class`, or zero.
fn held(ledger: &Ledger, class: ResourceClass) -> u64 {
    ledger.census().held.get(&class).copied().unwrap_or(0)
}

/// The corpus with `alpha` replaced and `epsilon` gone, published as a new base over the old composition.
fn publish_replacement(
    fixture: &Fixture,
    corpus: &[(&str, Vec<f32>)],
    sequence: u64,
) -> (Vec<(&'static str, Vec<f32>)>, VerifiedVectors) {
    let mut replaced: Vec<(&'static str, Vec<f32>)> = corpus
        .iter()
        .map(|(object, vector)| {
            let object: &'static str = OBJECTS.iter().copied().find(|o| o == object).unwrap();
            (object, vector.clone())
        })
        .collect();
    replaced[0].1 = axis(7);
    replaced.retain(|(object, _)| *object != "epsilon");
    let new_base = fixture.layer_from(&export(&replaced, &[], 10 * sequence as i64));
    fixture
        .publish(&fixture.compose(sequence, &new_base, &[]).unwrap())
        .unwrap();
    (replaced, new_base)
}

fn rank_view(
    fixture: &Fixture,
    projection: &Projection,
    view: &PinnedVectors,
    query: &[f32],
    k: usize,
) -> Result<LayeredRanking, RankRefusal> {
    let expected = fixture.expected();
    rank_expecting(fixture, projection, view, &expected, query, k)
}

fn rank_expecting(
    fixture: &Fixture,
    projection: &Projection,
    view: &PinnedVectors,
    expected: &ExpectedVectors<'_>,
    query: &[f32],
    k: usize,
) -> Result<LayeredRanking, RankRefusal> {
    let request = RankRequest {
        expected,
        query,
        authority: projection.authority(),
        bounds: oracle_bounds(k),
        max_entries: NonZeroUsize::new(64).unwrap(),
    };
    projection
        .store
        .with_conn(|conn| {
            Ok(rank(
                view,
                conn,
                &projection.kernel,
                &request,
                &EvalBudget::unbounded(),
                &fixture.admission,
            ))
        })
        .unwrap()
}

fn keyed(ranking: &LayeredRanking) -> Vec<(String, f64)> {
    ranking
        .ranking
        .ranked
        .iter()
        .map(|row| (row.occurrence_id.clone(), row.score))
        .collect()
}

fn map(rows: &[(&str, Vec<f32>)]) -> Vec<(String, Vec<f32>)> {
    rows.iter()
        .map(|(object, vector)| (occurrence_id(object), vector.clone()))
        .collect()
}

/// The bytes the view should keep resident: the store's manifests, summed through the resident set `VerifiedVectors` publishes rather than a list the reader keeps.
fn resident_bytes(fixture: &Fixture, members: &[String]) -> u64 {
    members
        .iter()
        .map(|digest| {
            daemon::vector_generation::resident_bytes(&fixture.store.manifest(digest).unwrap())
        })
        .sum()
}

/// Every manifest byte of `digests`, from the store rather than the reader.
fn manifest_bytes(fixture: &Fixture, digests: &[String]) -> u64 {
    digests
        .iter()
        .flat_map(|digest| fixture.store.manifest(digest).unwrap().files)
        .map(|file| file.size)
        .sum()
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_view_pins_the_record_and_every_member_reads_rows_and_codes_by_offset_and_ranks_from_the_files()
{
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let corpus = corpus();
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    let low = unit([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    let delta = fixture.layer_from(&export(&[("alpha", low.clone())], &["beta"], 12));
    let composition = fixture.compose(1, &base, &[delta]).unwrap();
    let digest = fixture.publish(&composition).unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();

    assert_eq!(view.digest(), digest);
    assert_eq!(view.members(), composition.members());
    for member in view.members().iter().chain([&digest]) {
        assert!(
            !try_exclusive(&fixture.generation_dir(member)),
            "a competing exclusive lock on a pinned generation fails"
        );
    }
    // Exactly the resident tables are held in the resident pool, and every pinned byte is in the census.
    let resident = resident_bytes(&fixture, &composition.members());
    assert!(resident > 0);
    let census = fixture.ledger.census();
    assert_eq!(census.resident, resident);
    assert_eq!(census.held[&ResourceClass::LayerTables], resident);
    let mut pinned = composition.members();
    pinned.push(digest.clone());
    assert_eq!(census.pinned, manifest_bytes(&fixture, &pinned));
    assert_eq!(
        census.disk, 0,
        "pinned generations are recorded, not reserved"
    );

    // Rows come back bit for bit from their offsets; codes are the rows under this layer's own scales.
    let layout = view.layout();
    for (layer, expected) in view
        .layers()
        .iter()
        .zip([&corpus[..], &[("alpha", low.clone())][..]])
    {
        let mut expected: Vec<(String, Vec<f32>)> = map(expected);
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            layer.occurrence_ids(),
            expected
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>()
        );
        for (index, (_, vector)) in expected.iter().enumerate() {
            assert_eq!(layer.row(index).unwrap(), *vector);
            assert_eq!(
                layer.codes(index).unwrap(),
                scalar::encode(&layout, &layer.scales, vector)
                    .unwrap()
                    .codes
            );
        }
        assert!(
            layer.row(expected.len()).is_err(),
            "no read leaves the declared rows"
        );
        assert!(layer.codes(expected.len()).is_err());
    }

    let query = axis(0);
    let ranking = rank_view(&fixture, &projection, &view, &query, 8).unwrap();
    let mut expected = map(&corpus);
    expected.retain(|(id, _)| *id != occurrence_id("beta"));
    for (id, vector) in &mut expected {
        if *id == occurrence_id("alpha") {
            *vector = low.clone();
        }
    }
    assert_eq!(keyed(&ranking), reference(&query, &expected));
    // `beta` is live in the projection and masked by the delta: a shortfall, not a fallback to the base's row.
    assert_eq!(
        ranking.ranking.completion,
        Completion::Incomplete(IncompleteReason::DenseCoverageShortfall)
    );
    assert_eq!(ranking.ranking.coverage.required, 5);
    assert_eq!(ranking.ranking.coverage.missing_without_pending, 1);
    assert_eq!(
        ranking.layers,
        LayerAccount {
            winners: 4,
            superseded: 1,
            masked: 1,
            revoked: 0,
            unvisited: 0
        }
    );
    assert_eq!(
        held(&fixture.ledger, ResourceClass::Scratch),
        0,
        "the page scratch is released when the walk returns"
    );

    drop(view);
    assert_eq!(
        fixture.ledger.census(),
        daemon::vector_admission::Census::default(),
        "dropping the view releases its bytes"
    );
    for member in composition.members().iter().chain([&digest]) {
        assert!(try_exclusive(&fixture.generation_dir(member)));
    }
}

#[test]
fn identity_handoff_allocation_contract() {
    let (_, canary) = alloc_recorder::record_window(|| {
        let bytes = std::hint::black_box(vec![0u8; 123]);
        drop(std::hint::black_box(bytes));
    });
    assert!(!canary.overflow);
    assert_eq!(canary.allocation_events, 1);
    assert_eq!(canary.requested_bytes, 123);
    assert_eq!(canary.live_bytes_at_close, 0);

    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let checkpoint = base.sidecar.checkpoint();
    let query = axis(0);
    let budget = EvalBudget::unbounded();
    // A revoked grant refuses the page scratch after every identity check and allocates nothing on the way; a limit denial would name the limit in a `String`.
    let revoked = Admission {
        invalidated: CancellationToken::new(),
        ..fixture.admission.clone()
    };
    revoked.invalidated.cancel();
    for checkpoint in [None, Some(&checkpoint)] {
        let expected = ExpectedVectors {
            checkpoint,
            ..fixture.expected()
        };
        let request = RankRequest {
            expected: &expected,
            query: &query,
            authority: projection.authority(),
            bounds: oracle_bounds(8),
            max_entries: NonZeroUsize::new(64).unwrap(),
        };
        let (result, ledger) = projection
            .store
            .with_conn(|conn| {
                Ok(alloc_recorder::record_window(|| {
                    rank(&view, conn, &projection.kernel, &request, &budget, &revoked)
                }))
            })
            .unwrap();
        assert_eq!(
            result.unwrap_err(),
            RankRefusal::Scratch {
                bytes: PAGE_SCRATCH,
                refusal: Refusal::Denied(Denial::Invalidated)
            }
        );
        assert!(!ledger.overflow);
        assert_eq!(ledger.live_bytes_at_close, 0);
        eprintln!(
            "identity checkpoint={}: allocations={} requested={} peak={} live={}",
            checkpoint.is_some(),
            ledger.allocation_events,
            ledger.requested_bytes,
            ledger.peak_live_bytes,
            ledger.live_bytes_at_close,
        );
        assert_eq!(ledger.allocation_events, 0);
        assert_eq!(ledger.requested_bytes, 0);
        assert_eq!(ledger.peak_live_bytes, 0);
    }
}

#[test]
fn acquisition_frees_the_transaction_lock_before_hashing_and_a_late_refusal_hands_nothing_out() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    let delta = fixture.layer_from(&export(&[("alpha", axis(7))], &[], 12));
    let composition = fixture.compose(1, &base, &[delta]).unwrap();
    let digest = fixture.publish(&composition).unwrap();
    let members = composition.members();
    let lock = transaction_lock(&fixture);
    let selector = fixture.lifecycle_dir().join(VECTOR_PROFILE_NAME);
    let mut seen = Vec::new();
    fixture.release_transaction();
    let refusal = {
        let mut observe = |event: AcquireEvent| {
            seen.push(event);
            // The transaction lock is free at every window: a reader never holds mutators off while it hashes or hands off; its pins and both reservations are what keep the files.
            assert!(
                try_exclusive(&lock),
                "a mutator can take the transaction lock at {event:?}"
            );
            for pinned in members.iter().chain([&digest]) {
                assert!(
                    !try_exclusive(&fixture.generation_dir(pinned)),
                    "{pinned} is pinned at {event:?}"
                );
            }
            assert!(held(&fixture.ledger, ResourceClass::LayerTables) > 0);
            assert!(fixture.ledger.census().pinned > 0);
            if event != AcquireEvent::BeforeSelectorRecheck {
                // A mutator's exclusive transaction succeeds through the real API, and its prune reclaims nothing the view needs; the files are witnessed on disk afterwards.
                let transaction =
                    LifecycleTransactionLock::acquire_exclusive(Some(fixture.root.path()))
                        .expect("a mutator is not held off by a reader");
                let report = GenerationStore::open(Some(fixture.root.path()))
                    .unwrap()
                    .prune(&BTreeSet::new())
                    .unwrap();
                drop(transaction);
                assert_eq!(report.removed_generations, 0);
                let present = fixture.generations();
                assert!(members.iter().chain([&digest]).all(|d| present.contains(d)));
            }
            if event == AcquireEvent::BeforeSelectorRecheck {
                // The last check before handoff fails: the selector no longer names what acquisition observed.
                std::fs::remove_file(&selector).unwrap();
            }
        };
        acquire(
            Some(fixture.root.path()),
            &fixture.expected(),
            bounds(),
            &fixture.ledger,
            &fixture.admission,
            &mut observe,
        )
        .unwrap_err()
    };
    assert_eq!(
        refusal,
        AcquireRefusal::SelectorMoved {
            observed: daemon::vector_composition::SelectorState::Current,
            current: CurrentProfile::Absent
        }
    );
    assert_eq!(
        seen,
        vec![
            AcquireEvent::BeforeVerification,
            AcquireEvent::BeforeLastLayer,
            AcquireEvent::BeforeSelectorRecheck
        ]
    );
    assert!(
        try_exclusive(&lock),
        "the shared protection is released with the failure"
    );
    for pinned in members.iter().chain([&digest]) {
        assert!(
            try_exclusive(&fixture.generation_dir(pinned)),
            "no pin outlives a failed acquisition"
        );
    }
    assert_eq!(
        fixture.ledger.census(),
        daemon::vector_admission::Census::default(),
        "no reservation outlives a failed acquisition"
    );
    fixture.reacquire_transaction();
}

#[test]
fn a_publisher_that_runs_while_a_reader_verifies_moves_the_selector_and_the_reader_refuses() {
    let mut fixture = Fixture::new();
    let corpus = corpus();
    let old_base = fixture.layer_from(&export(&corpus, &[], 10));
    let old = fixture.compose(1, &old_base, &[]).unwrap();
    let old_digest = fixture.publish(&old).unwrap();
    // The replacement is staged while the fixture still holds the transaction; only its publication waits for the reader's verification window.
    let mut replaced = corpus.clone();
    replaced[0].1 = axis(7);
    let new_base = fixture.layer_from(&export(&replaced, &[], 20));
    let new = fixture.compose(2, &new_base, &[]).unwrap();
    let mut published = None;
    fixture.release_transaction();
    let refusal = {
        let mut observe = |event: AcquireEvent| {
            if event == AcquireEvent::BeforeVerification {
                let transaction =
                    LifecycleTransactionLock::acquire_exclusive(Some(fixture.root.path()))
                        .expect("a publisher is not held off by a verifying reader");
                let staging = Staging {
                    store: &fixture.store,
                    transaction: &transaction,
                    gate: &fixture.gate,
                    admission: &fixture.admission,
                    identity: &fixture.identity,
                    ledger: &fixture.ledger,
                    protected: &fixture.protected,
                };
                published = Some(
                    publish(&new, &staging, &fixture.work_dir(), &mut |_| Ok(()))
                        .map_err(|failure| failure.refusal)
                        .unwrap(),
                );
            }
        };
        acquire(
            Some(fixture.root.path()),
            &fixture.expected(),
            bounds(),
            &fixture.ledger,
            &fixture.admission,
            &mut observe,
        )
        .unwrap_err()
    };
    let new_digest = published.expect("the publisher ran during verification");
    assert_eq!(
        refusal,
        AcquireRefusal::SelectorMoved {
            observed: daemon::vector_composition::SelectorState::Current,
            current: CurrentProfile::Current(new_digest.clone())
        }
    );
    assert!(
        try_exclusive(&fixture.generation_dir(&old_digest)),
        "the superseded record is not left pinned"
    );
    assert_eq!(
        fixture.ledger.census(),
        daemon::vector_admission::Census::default(),
        "no reservation outlives the refused acquisition"
    );
    fixture.reacquire_transaction();

    // The retry takes the set the publisher left.
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    assert_eq!(view.digest(), new_digest);
    assert_eq!(view.members(), vec![new_base.digest.clone()]);
}

#[test]
fn no_composition_or_a_short_resident_limit_refuses_before_any_layer_and_a_truncated_row_is_unreadable()
 {
    let mut empty = Fixture::new();
    assert!(matches!(
        acquire_view(&mut empty, &mut |_| {}).unwrap_err(),
        AcquireRefusal::Unavailable(_)
    ));

    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let corpus = corpus();
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    let composition = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&composition).unwrap();
    let mut seen = 0;
    fixture.set_limit(RESIDENT_LIMIT, 16);
    let resident = resident_bytes(&fixture, std::slice::from_ref(&base.digest));
    let refusal = acquire_view(&mut fixture, &mut |_| seen += 1).unwrap_err();
    assert_eq!(
        refusal,
        AcquireRefusal::Reservation(Refusal::Denied(Denial::LimitExceeded {
            limit: RESIDENT_LIMIT.to_owned(),
            observed: resident,
            max: 16
        }))
    );
    assert_eq!(seen, 0);
    assert!(try_exclusive(&fixture.generation_dir(&base.digest)));
    assert_eq!(
        fixture.ledger.census(),
        daemon::vector_admission::Census::default()
    );
    fixture.set_limit(RESIDENT_LIMIT, u64::MAX);

    // A row artifact cut short after acquisition, as a fault injector and not a threat-model claim: the row past the cut is unreadable, and nothing is reinterpreted.
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let rows = fixture.generation_dir(&base.digest).join(ROWS_FILE);
    let bytes = std::fs::read(&rows).unwrap();
    std::fs::write(&rows, &bytes[..bytes.len() - 8]).unwrap();
    let layer = &view.layers()[0];
    assert!(layer.row(3).is_ok());
    assert!(matches!(layer.row(4), Err(RowFault::Unavailable(_))));
    let refusal = rank_view(&fixture, &projection, &view, &axis(0), 8).unwrap_err();
    assert!(
        matches!(
            refusal,
            RankRefusal::Layered(LayeredRefusal::Oracle(OracleRefusal::Unreadable { .. }))
        ),
        "{refusal:?}"
    );
    assert_eq!(held(&fixture.ledger, ResourceClass::Scratch), 0);
}

#[test]
fn old_readers_keep_their_complete_set_while_a_new_composition_is_published_and_pruned() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let corpus = corpus();
    let old_base = fixture.layer_from(&export(&corpus, &[], 10));
    let old = fixture.compose(1, &old_base, &[]).unwrap();
    let old_digest = fixture.publish(&old).unwrap();
    let old_view = acquire_view(&mut fixture, &mut |_| {}).unwrap();

    let (replaced, new_base) = publish_replacement(&fixture, &corpus, 2);
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_generations, 0);
    assert_eq!(
        report.retained_pinned, 1,
        "the superseded record is pinned by the old reader"
    );
    assert_eq!(
        report.retained_bytes,
        manifest_bytes(&fixture, std::slice::from_ref(&old_digest))
    );
    // The prune's readback is within what the ledger knows readers pin: the record and its base.
    let reconciliation = fixture.ledger.reconcile(&report);
    assert_eq!(
        reconciliation.ledger_pinned,
        manifest_bytes(&fixture, &[old_digest.clone(), old_base.digest.clone()])
    );
    assert!(!reconciliation.unaccounted());

    let new_view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    assert_eq!(old_view.digest(), old_digest);
    assert_eq!(new_view.members(), vec![new_base.digest.clone()]);
    let query = axis(0);
    let old_ranking = rank_view(&fixture, &projection, &old_view, &query, 8).unwrap();
    assert_eq!(keyed(&old_ranking), reference(&query, &map(&corpus)));
    assert_eq!(old_ranking.ranking.completion, Completion::Complete);
    let new_ranking = rank_view(&fixture, &projection, &new_view, &query, 8).unwrap();
    assert_eq!(keyed(&new_ranking), reference(&query, &map(&replaced)));
    assert_eq!(
        new_ranking.ranking.completion,
        Completion::Incomplete(IncompleteReason::DenseCoverageShortfall)
    );

    drop(old_view);
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!((report.removed_generations, report.retained_pinned), (1, 0));
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(
        report.removed_generations, 1,
        "the old base follows its record"
    );
    assert!(!fixture.generations().contains(&old_digest));
    assert!(!fixture.generations().contains(&old_base.digest));
    assert!(!try_exclusive(&fixture.generation_dir(&new_base.digest)));
    assert_eq!(
        keyed(&rank_view(&fixture, &projection, &new_view, &query, 8).unwrap()),
        reference(&query, &map(&replaced))
    );
}

#[test]
fn handoff_rechecks_every_binding_and_a_hidden_or_retired_winner_never_falls_back() {
    let mut fixture = Fixture::new();
    let admitted: Vec<&str> = OBJECTS.iter().copied().filter(|o| *o != "gamma").collect();
    let projection = projection(&fixture, &admitted);
    let corpus = corpus();
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    let delta = fixture.layer_from(&export(&[("alpha", axis(0)), ("gamma", axis(0))], &[], 12));
    let composition = fixture.compose(1, &base, &[delta]).unwrap();
    fixture.publish(&composition).unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let query = axis(0);

    let mut other_model = fixture.generation.clone();
    other_model.embedding_model = "another-model".to_owned();
    let mut other_generation = fixture.generation.clone();
    other_generation.generation_id = "gen-vectors-2".to_owned();
    for (generation, field) in [
        (&other_model, "embedding_model"),
        (&other_generation, "generation_id"),
    ] {
        let foreign = ExpectedVectors {
            generation,
            ..fixture.expected()
        };
        let refusal =
            rank_expecting(&fixture, &projection, &view, &foreign, &query, 8).unwrap_err();
        assert_eq!(refusal, RankRefusal::Identity { field });
    }
    let other_recipe = ExpectedVectors {
        unit_norm_tolerance: 1e-2,
        ..fixture.expected()
    };
    assert_eq!(
        rank_expecting(&fixture, &projection, &view, &other_recipe, &query, 8).unwrap_err(),
        RankRefusal::Identity {
            field: "unit_norm_tolerance"
        }
    );
    // The resident limit leaves room for the view's tables but not for one page of scratch.
    let resident = fixture.ledger.census().resident;
    fixture.set_limit(RESIDENT_LIMIT, resident + PAGE_SCRATCH - 1);
    let refusal = rank_view(&fixture, &projection, &view, &query, 8).unwrap_err();
    assert_eq!(
        refusal,
        RankRefusal::Scratch {
            bytes: PAGE_SCRATCH,
            refusal: Refusal::Denied(Denial::LimitExceeded {
                limit: RESIDENT_LIMIT.to_owned(),
                observed: resident + PAGE_SCRATCH,
                max: resident + PAGE_SCRATCH - 1
            })
        }
    );
    fixture.set_limit(RESIDENT_LIMIT, resident + PAGE_SCRATCH);
    assert!(
        rank_view(&fixture, &projection, &view, &query, 8).is_ok(),
        "the exact bound admits"
    );
    fixture.set_limit(RESIDENT_LIMIT, u64::MAX);

    // A row bound below the page size caps every page, so the charge is for the rows a page can hold, not the nominal page.
    // Room for one row plus the raw row, and not for the nominal thousand-row page.
    fixture.set_limit(RESIDENT_LIMIT, resident + (1 + 1) * 8 * 4);
    let expected = fixture.expected();
    let request = RankRequest {
        expected: &expected,
        query: &query,
        authority: projection.authority(),
        bounds: OracleBounds {
            page_rows: NonZeroUsize::new(1000).unwrap(),
            max_rows: NonZeroUsize::new(1).unwrap(),
            ..oracle_bounds(8)
        },
        max_entries: NonZeroUsize::new(64).unwrap(),
    };
    let ranking = projection
        .store
        .with_conn(|conn| {
            Ok(rank(
                &view,
                conn,
                &projection.kernel,
                &request,
                &EvalBudget::unbounded(),
                &fixture.admission,
            ))
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        ranking.ranking.completion,
        Completion::Incomplete(IncompleteReason::RowBound)
    );
    assert_eq!(held(&fixture.ledger, ResourceClass::Scratch), 0);
    fixture.set_limit(RESIDENT_LIMIT, u64::MAX);

    for page_rows in [usize::MAX, usize::MAX / (8 * 4)] {
        let expected = fixture.expected();
        let request = RankRequest {
            expected: &expected,
            query: &query,
            authority: projection.authority(),
            bounds: OracleBounds {
                page_rows: NonZeroUsize::new(page_rows).unwrap(),
                max_rows: NonZeroUsize::new(usize::MAX).unwrap(),
                ..oracle_bounds(8)
            },
            max_entries: NonZeroUsize::new(64).unwrap(),
        };
        let refusal = projection
            .store
            .with_conn(|conn| {
                Ok(rank(
                    &view,
                    conn,
                    &projection.kernel,
                    &request,
                    &EvalBudget::unbounded(),
                    &fixture.admission,
                ))
            })
            .unwrap()
            .unwrap_err();
        assert_eq!(
            refusal,
            RankRefusal::Scratch {
                bytes: u64::MAX,
                refusal: Refusal::Overflow {
                    pool: Pool::Resident
                }
            }
        );
        assert_eq!(held(&fixture.ledger, ResourceClass::Scratch), 0);
    }

    // `gamma` is hidden by the kernel, and `alpha` is retired after the view was taken: neither the delta's row nor the base's row for them is returned.
    projection.retire("alpha");
    let ranking = rank_view(&fixture, &projection, &view, &query, 8).unwrap();
    let mut expected = map(&corpus);
    expected.retain(|(id, _)| *id != occurrence_id("alpha") && *id != occurrence_id("gamma"));
    assert_eq!(keyed(&ranking), reference(&query, &expected));
    assert_eq!(ranking.ranking.completion, Completion::Complete);
    let mut excluded = ranking.ranking.consumed.excluded.clone();
    excluded.sort_by_key(|(verdict, _)| format!("{verdict:?}"));
    assert_eq!(
        excluded,
        vec![
            (EligibilityVerdict::Hidden, 1),
            (EligibilityVerdict::Retracted, 1)
        ]
    );
    assert_eq!(ranking.ranking.coverage.with_vector, 5);
}

/// Everything a blocking worker owns for one ranking: the view, the projection, the kernel, the expectation's parts, and its grant.
struct Work {
    view: Arc<PinnedVectors>,
    projection_store: Arc<storage::SqliteStore>,
    kernel: Arc<kernel::KernelStore>,
    generation: retrieval::batch::VectorGeneration,
    kernel_incarnation_id: String,
    project: kernel::ProjectScope,
    grant: Admission,
}

impl Work {
    /// The view is held until this returns, whatever happened to whoever spawned it.
    fn run(self, budget: &EvalBudget) -> Result<LayeredRanking, RankRefusal> {
        let expected = ExpectedVectors {
            generation: &self.generation,
            kernel_incarnation_id: &self.kernel_incarnation_id,
            metric: retrieval::dense::Metric::InnerProduct,
            unit_norm_tolerance: support::vector_store::TOLERANCE,
            recipe: retrieval::dense::scalar::ScalarRecipe::SymmetricInt8V1,
            checkpoint: None,
        };
        let request = RankRequest {
            expected: &expected,
            query: &axis(0),
            authority: retrieval::eligibility::Authority {
                project: &self.project,
                destination: kernel::ArtifactDestination::Local,
            },
            bounds: oracle_bounds(8),
            max_entries: NonZeroUsize::new(64).unwrap(),
        };
        self.projection_store
            .with_conn(|conn| {
                Ok(rank(
                    &self.view,
                    conn,
                    &self.kernel,
                    &request,
                    budget,
                    &self.grant,
                ))
            })
            .unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_worker_owns_the_view_and_its_charges_until_the_read_returns_whatever_the_caller_does() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let corpus = corpus();
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    let composition = fixture.compose(1, &base, &[]).unwrap();
    let digest = fixture.publish(&composition).unwrap();
    let generation = fixture.generation.clone();
    let kernel_incarnation_id = fixture.identity.kernel_incarnation_id.clone();
    let grant = fixture.admission.clone();
    let work = |view: Arc<PinnedVectors>| Work {
        view,
        projection_store: Arc::clone(&projection.store),
        kernel: Arc::clone(&projection.kernel),
        generation: generation.clone(),
        kernel_incarnation_id: kernel_incarnation_id.clone(),
        project: projection.project.clone(),
        grant: grant.clone(),
    };

    // The caller drops its handle and its Arc while the worker waits to start the read; the view, its pins, and its bytes stay with the work.
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let (release, gate) = mpsc::channel::<()>();
    let (entered, entering) = mpsc::channel::<()>();
    let owned = work(Arc::clone(&view));
    let handle = tokio::task::spawn_blocking(move || {
        entered.send(()).unwrap();
        gate.recv().unwrap();
        owned.run(&EvalBudget::unbounded())
    });
    drop(view);
    drop(handle);
    entering.recv().unwrap();
    assert!(!try_exclusive(&fixture.generation_dir(&digest)));
    let old_resident = held(&fixture.ledger, ResourceClass::LayerTables);
    assert!(old_resident > 0);
    // A publisher and a prune meanwhile leave the worker's set alone, and a new reader takes the new complete set while the old work is still held.
    let (replaced, new_base) = publish_replacement(&fixture, &corpus, 2);
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!((report.removed_generations, report.retained_pinned), (0, 1));
    let overlapping = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    assert_eq!(overlapping.members(), vec![new_base.digest.clone()]);
    assert!(
        held(&fixture.ledger, ResourceClass::LayerTables) > old_resident,
        "both views' tables are held at once"
    );
    let overlapping_ranking = rank_view(&fixture, &projection, &overlapping, &axis(0), 8).unwrap();
    assert_eq!(
        keyed(&overlapping_ranking),
        reference(&axis(0), &map(&replaced))
    );
    release.send(()).unwrap();
    let store_dir = fixture.generation_dir(&digest);
    let probe = Arc::clone(&fixture.ledger);
    let overlapping_resident = resident_bytes(&fixture, &overlapping.members());
    tokio::task::spawn_blocking(move || {
        wait_until("the detached worker to release the old view", || {
            try_exclusive(&store_dir)
                && held(&probe, ResourceClass::LayerTables) == overlapping_resident
        });
    })
    .await
    .unwrap();
    assert_eq!(held(&fixture.ledger, ResourceClass::Scratch), 0);
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(
        report.removed_generations, 1,
        "the old record goes; the new reader's set stays"
    );
    assert!(!try_exclusive(&fixture.generation_dir(&new_base.digest)));
    drop(overlapping);
    assert_eq!(
        fixture.ledger.census(),
        daemon::vector_admission::Census::default()
    );

    // Completion with retained output: whoever keeps the ranking's view keeps its pins until that view is dropped.
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let new_digest = view.digest().to_owned();
    let owned = work(Arc::clone(&view));
    let ranked = tokio::task::spawn_blocking(move || owned.run(&EvalBudget::unbounded()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(keyed(&ranked), reference(&axis(0), &map(&replaced)));
    assert!(!try_exclusive(&fixture.generation_dir(&new_digest)));
    assert!(held(&fixture.ledger, ResourceClass::LayerTables) > 0);
    drop(view);
    assert!(try_exclusive(&fixture.generation_dir(&new_digest)));
    assert_eq!(
        fixture.ledger.census(),
        daemon::vector_admission::Census::default()
    );

    // A cancelled budget, or one whose deadline has passed, ends the walk without a ranking; the view is held through the read either way and released only by its owner.
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let cancelled = EvalBudget::unbounded();
    cancelled.cancel();
    let expired = EvalBudget::new(
        Some(Instant::now() - Duration::from_millis(1)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    for budget in [cancelled, expired] {
        let owned = work(Arc::clone(&view));
        let outcome = tokio::task::spawn_blocking(move || owned.run(&budget))
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            Err(RankRefusal::Layered(LayeredRefusal::Oracle(
                OracleRefusal::BudgetExhausted
            )))
        ));
        assert!(
            !try_exclusive(&fixture.generation_dir(&new_digest)),
            "the caller's view is untouched"
        );
        assert!(held(&fixture.ledger, ResourceClass::LayerTables) > 0);
    }
    drop(view);
    assert!(try_exclusive(&fixture.generation_dir(&new_digest)));
    assert_eq!(
        fixture.ledger.census(),
        daemon::vector_admission::Census::default()
    );
}

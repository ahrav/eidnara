//! The file-backed reader over a real lifecycle store and a real projection: pins witnessed by competing flocks, rows read by offset against an independent f64 reference, and ownership that outlives the caller.

mod support;

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use daemon::vector_reader::{
    AcquireEvent, AcquireRefusal, PinnedVectors, RankRefusal, RankRequest, ReaderBounds,
    WorkerRequest, acquire, rank, rank_on_worker,
};
use host_runtime::generation::{CurrentProfile, GenerationStore, VECTOR_PROFILE_NAME};
use host_runtime::lifecycle::{TRANSACTION_LOCK_NAME, coordination_dir_path};
use host_runtime::wire::ByteBudget;
use kernel::applicability::EvalBudget;
use kernel::{ArtifactDestination, EligibilityVerdict};
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::export::{ExportedRow, LiveRows};
use retrieval::dense::scalar;
use retrieval::dense::{Completion, IncompleteReason, LayerAccount, OracleBounds, RowAccess};
use support::dense_projection::{Projection, occurrence_id, reference};
use support::vector_store::{Fixture, unit};

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

/// A competing exclusive flock attempt on `path`, released at once if it succeeds; `false` means someone holds the file.
fn try_flock_exclusive(path: &Path) -> bool {
    let file = std::fs::File::open(path).expect("lock target opens");
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if locked {
        unsafe {
            libc::flock(file.as_raw_fd(), libc::LOCK_UN);
        }
    }
    locked
}

fn transaction_lock(fixture: &Fixture) -> std::path::PathBuf {
    coordination_dir_path(Some(fixture.root.path()))
        .unwrap()
        .join(TRANSACTION_LOCK_NAME)
}

fn acquire_view(
    fixture: &mut Fixture,
    residency: &ByteBudget,
    observe: &mut dyn FnMut(AcquireEvent),
) -> Result<Arc<PinnedVectors>, AcquireRefusal> {
    fixture.release_transaction();
    let view = acquire(
        Some(fixture.root.path()),
        &fixture.expected(),
        bounds(),
        residency,
        observe,
    );
    fixture.reacquire_transaction();
    view
}

fn rank_view(
    fixture: &Fixture,
    projection: &Projection,
    view: &PinnedVectors,
    query: &[f32],
    k: usize,
    scratch: &ByteBudget,
) -> Result<retrieval::dense::LayeredRanking, RankRefusal> {
    let request = RankRequest {
        generation: &fixture.generation,
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
                scratch,
            ))
        })
        .unwrap()
}

fn keyed(ranking: &retrieval::dense::LayeredRanking) -> Vec<(String, f64)> {
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
    let projection = Projection::new(
        fixture.root.path(),
        &fixture.identity,
        &fixture.generation,
        &OBJECTS,
        &OBJECTS,
    );
    let corpus = corpus();
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    let low = unit([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    let delta = fixture.layer_from(&export(&[("alpha", low.clone())], &["beta"], 12));
    let composition = fixture.compose(1, &base, &[delta]).unwrap();
    let digest = fixture.publish(&composition).unwrap();
    let residency = ByteBudget::new(1 << 20);
    let view = acquire_view(&mut fixture, &residency, &mut |_| {}).unwrap();

    assert_eq!(view.digest, digest);
    assert_eq!(view.members(), composition.members());
    for member in view.members().iter().chain([&digest]) {
        assert!(
            !try_flock_exclusive(&fixture.generation_dir(member)),
            "a competing exclusive lock on a pinned generation fails"
        );
    }
    assert!(
        residency.try_charge(1 << 20).is_none(),
        "the resident bytes are charged while the view lives"
    );

    // Rows come back bit for bit from their offsets; codes are the rows under this layer's own scales.
    let layout = view.layout;
    for (layer, expected) in view
        .layers
        .iter()
        .zip([&corpus[..], &[("alpha", low.clone())][..]])
    {
        let mut expected: Vec<(String, Vec<f32>)> = map(expected);
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            layer.occurrence_ids,
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
    let scratch = ByteBudget::new(1 << 16);
    let ranking = rank_view(&fixture, &projection, &view, &query, 8, &scratch).unwrap();
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
    assert!(
        scratch.try_charge(1 << 16).is_some(),
        "the page scratch is released when the walk returns"
    );

    drop(view);
    assert!(
        residency.try_charge(1 << 20).is_some(),
        "dropping the view releases its bytes"
    );
    for member in composition.members().iter().chain([&digest]) {
        assert!(try_flock_exclusive(&fixture.generation_dir(member)));
    }
}

#[test]
fn acquisition_holds_the_shared_protection_and_a_failed_final_open_hands_nothing_out() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    let delta = fixture.layer_from(&export(&[("alpha", axis(7))], &[], 12));
    let composition = fixture.compose(1, &base, &[delta]).unwrap();
    fixture.publish(&composition).unwrap();
    let members = composition.members();
    let residency = ByteBudget::new(1 << 20);
    let lock = transaction_lock(&fixture);
    let last = members.last().unwrap().clone();
    let last_dir = fixture.generation_dir(&last);
    let mut seen = Vec::new();
    fixture.release_transaction();
    let refusal = {
        let mut observe = |event: AcquireEvent| {
            seen.push(event);
            if event == AcquireEvent::BeforeLastOpen {
                // Every pin is held: a publisher's exclusive lifecycle lock cannot be taken, and no member can be reclaimed.
                assert!(
                    !try_flock_exclusive(&lock),
                    "a competing mutator is refused the transaction lock"
                );
                for member in &members {
                    assert!(!try_flock_exclusive(&fixture.generation_dir(member)));
                }
                let report = GenerationStore::open(Some(fixture.root.path()))
                    .unwrap()
                    .prune(&BTreeSet::new())
                    .unwrap();
                assert_eq!(report.removed_generations, 0);
                // The last file to open is made to fail its hash.
                let path = last_dir.join(daemon::vector_generation::CODES_FILE);
                let mut bytes = std::fs::read(&path).unwrap();
                bytes[0] ^= 0xff;
                std::fs::write(&path, bytes).unwrap();
            }
        };
        acquire(
            Some(fixture.root.path()),
            &fixture.expected(),
            bounds(),
            &residency,
            &mut observe,
        )
        .unwrap_err()
    };
    assert!(matches!(refusal, AcquireRefusal::Store(_)), "{refusal:?}");
    assert_eq!(seen, vec![AcquireEvent::BeforeLastOpen]);
    assert!(
        try_flock_exclusive(&lock),
        "the shared protection is released with the failure"
    );
    for member in &members {
        assert!(
            try_flock_exclusive(&fixture.generation_dir(member)),
            "no pin outlives a failed acquisition"
        );
    }
    assert!(
        residency.try_charge(1 << 20).is_some(),
        "no charge outlives a failed acquisition"
    );
}

#[test]
fn a_selector_that_moves_before_the_recheck_refuses_the_view() {
    let mut fixture = Fixture::new();
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    let composition = fixture.compose(1, &base, &[]).unwrap();
    let digest = fixture.publish(&composition).unwrap();
    let residency = ByteBudget::new(1 << 20);
    let selector = fixture.lifecycle_dir().join(VECTOR_PROFILE_NAME);
    let refusal = acquire_view(&mut fixture, &residency, &mut |event| {
        if event == AcquireEvent::BeforeSelectorRecheck {
            std::fs::remove_file(&selector).unwrap();
        }
    })
    .unwrap_err();
    assert_eq!(
        refusal,
        AcquireRefusal::SelectorMoved {
            observed: daemon::vector_composition::SelectorState::Current,
            current: CurrentProfile::Absent
        }
    );
    assert!(try_flock_exclusive(&fixture.generation_dir(&digest)));
    assert!(residency.try_charge(1 << 20).is_some());
    // Nothing published: no view.
    let empty = Fixture::new();
    let mut empty = empty;
    assert!(matches!(
        acquire_view(&mut empty, &residency, &mut |_| {}).unwrap_err(),
        AcquireRefusal::Unavailable(_)
    ));
    // A residency budget too small for the resident tables refuses before any file is opened for reading.
    let mut seen = 0;
    let refusal = acquire_view(&mut fixture, &ByteBudget::new(16), &mut |_| seen += 1).unwrap_err();
    assert!(
        matches!(refusal, AcquireRefusal::Residency { .. }),
        "{refusal:?}"
    );
    assert_eq!(seen, 0);
}

#[test]
fn old_readers_keep_their_complete_set_while_a_new_composition_is_published_and_pruned() {
    let mut fixture = Fixture::new();
    let projection = Projection::new(
        fixture.root.path(),
        &fixture.identity,
        &fixture.generation,
        &OBJECTS,
        &OBJECTS,
    );
    let corpus = corpus();
    let old_base = fixture.layer_from(&export(&corpus, &[], 10));
    let old = fixture.compose(1, &old_base, &[]).unwrap();
    let old_digest = fixture.publish(&old).unwrap();
    let residency = ByteBudget::new(1 << 20);
    let old_view = acquire_view(&mut fixture, &residency, &mut |_| {}).unwrap();

    // The new base replaces `alpha`'s row and no longer holds `epsilon` at all.
    let mut replaced = corpus.clone();
    replaced[0].1 = axis(7);
    replaced.retain(|(object, _)| *object != "epsilon");
    let new_base = fixture.layer_from(&export(&replaced, &[], 20));
    let new = fixture.compose(2, &new_base, &[]).unwrap();
    let new_digest = fixture.publish(&new).unwrap();
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_generations, 0);
    assert_eq!(
        report.retained_pinned, 1,
        "the superseded record is pinned by the old reader"
    );
    let record_bytes: u64 = fixture
        .store
        .manifest(&old_digest)
        .unwrap()
        .files
        .iter()
        .map(|file| file.size)
        .sum();
    assert_eq!(report.retained_bytes, record_bytes);

    let new_view = acquire_view(&mut fixture, &residency, &mut |_| {}).unwrap();
    assert_eq!(
        (old_view.digest.as_str(), new_view.digest.as_str()),
        (old_digest.as_str(), new_digest.as_str())
    );
    let query = axis(0);
    let scratch = ByteBudget::new(1 << 16);
    let old_ranking = rank_view(&fixture, &projection, &old_view, &query, 8, &scratch).unwrap();
    assert_eq!(keyed(&old_ranking), reference(&query, &map(&corpus)));
    assert_eq!(old_ranking.ranking.completion, Completion::Complete);
    let new_ranking = rank_view(&fixture, &projection, &new_view, &query, 8, &scratch).unwrap();
    let expected = map(&replaced);
    assert_eq!(keyed(&new_ranking), reference(&query, &expected));
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
    assert!(!try_flock_exclusive(
        &fixture.generation_dir(&new_base.digest)
    ));
    assert_eq!(
        keyed(&rank_view(&fixture, &projection, &new_view, &query, 8, &scratch).unwrap()),
        reference(&query, &expected)
    );
}

#[test]
fn handoff_rechecks_the_binding_and_a_hidden_or_retired_winner_never_falls_back() {
    let mut fixture = Fixture::new();
    let admitted: Vec<&str> = OBJECTS.iter().copied().filter(|o| *o != "gamma").collect();
    let projection = Projection::new(
        fixture.root.path(),
        &fixture.identity,
        &fixture.generation,
        &OBJECTS,
        &admitted,
    );
    let corpus = corpus();
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    let delta = fixture.layer_from(&export(&[("alpha", axis(0)), ("gamma", axis(0))], &[], 12));
    let composition = fixture.compose(1, &base, &[delta]).unwrap();
    fixture.publish(&composition).unwrap();
    let residency = ByteBudget::new(1 << 20);
    let view = acquire_view(&mut fixture, &residency, &mut |_| {}).unwrap();
    let query = axis(0);
    let scratch = ByteBudget::new(1 << 16);

    let mut other = fixture.generation.clone();
    other.embedding_model = "another-model".to_owned();
    let request = RankRequest {
        generation: &other,
        query: &query,
        authority: projection.authority(),
        bounds: oracle_bounds(8),
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
                &scratch,
            ))
        })
        .unwrap()
        .unwrap_err();
    assert_eq!(
        refusal,
        RankRefusal::Identity {
            field: "embedding_model"
        }
    );
    let refusal =
        rank_view(&fixture, &projection, &view, &query, 8, &ByteBudget::new(8)).unwrap_err();
    assert_eq!(refusal, RankRefusal::Scratch { bytes: 2 * 8 * 4 });

    // `gamma` is hidden by the kernel, and `alpha` is retired after the view was taken: neither the delta's row nor the base's row for them is returned.
    projection.retire("alpha");
    let ranking = rank_view(&fixture, &projection, &view, &query, 8, &scratch).unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_worker_owns_the_view_and_its_charges_until_the_read_returns_whatever_the_caller_does() {
    let mut fixture = Fixture::new();
    let projection = Projection::new(
        fixture.root.path(),
        &fixture.identity,
        &fixture.generation,
        &OBJECTS,
        &OBJECTS,
    );
    let corpus = corpus();
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    let composition = fixture.compose(1, &base, &[]).unwrap();
    let digest = fixture.publish(&composition).unwrap();
    let residency = ByteBudget::new(1 << 20);
    let view = acquire_view(&mut fixture, &residency, &mut |_| {}).unwrap();
    let kernel = Arc::clone(&projection.kernel);
    let store = Arc::new(projection.store);
    let generation = fixture.generation.clone();
    let project = projection.project.clone();
    let request = || WorkerRequest {
        generation: generation.clone(),
        query: axis(0),
        project: project.clone(),
        destination: ArtifactDestination::Local,
        bounds: oracle_bounds(8),
        max_entries: NonZeroUsize::new(64).unwrap(),
    };
    let scratch = ByteBudget::new(1 << 16);

    // The caller drops its handle and its Arc while the worker waits on the projection read; the view, its pins, and its bytes stay.
    let (release, held) = mpsc::channel::<()>();
    let (entered, entering) = mpsc::channel::<()>();
    let worker_store = Arc::clone(&store);
    let handle = rank_on_worker(
        Arc::clone(&view),
        Arc::clone(&kernel),
        move |walk| {
            entered.send(()).unwrap();
            held.recv().unwrap();
            worker_store
                .with_conn(|conn| {
                    walk(conn);
                    Ok(())
                })
                .map_err(|error| error.to_string())
        },
        request(),
        EvalBudget::unbounded(),
        scratch.clone(),
    );
    drop(view);
    drop(handle);
    entering.recv().unwrap();
    assert!(!try_flock_exclusive(&fixture.generation_dir(&digest)));
    assert!(residency.try_charge(1 << 20).is_none());
    // A publisher and a prune meanwhile leave the worker's set alone.
    let mut replaced = corpus.clone();
    replaced[0].1 = axis(7);
    let new_base = fixture.layer_from(&export(&replaced, &[], 20));
    fixture
        .publish(&fixture.compose(2, &new_base, &[]).unwrap())
        .unwrap();
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!((report.removed_generations, report.retained_pinned), (0, 1));
    release.send(()).unwrap();
    let store_dir = fixture.generation_dir(&digest);
    tokio::task::spawn_blocking(move || {
        wait_until("the detached worker to release the old view", || {
            try_flock_exclusive(&store_dir)
        });
    })
    .await
    .unwrap();
    assert!(residency.try_charge(1 << 20).is_some());
    assert!(scratch.try_charge(1 << 16).is_some());
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_generations, 1);

    // Completion with retained output: the ranking keeps the new view until the output is dropped.
    let view = acquire_view(&mut fixture, &residency, &mut |_| {}).unwrap();
    let new_digest = view.digest.clone();
    let worker_store = Arc::clone(&store);
    let ranked = rank_on_worker(
        Arc::clone(&view),
        Arc::clone(&kernel),
        move |walk| {
            worker_store
                .with_conn(|conn| {
                    walk(conn);
                    Ok(())
                })
                .map_err(|error| error.to_string())
        },
        request(),
        EvalBudget::unbounded(),
        scratch.clone(),
    )
    .await
    .unwrap()
    .unwrap();
    drop(view);
    assert_eq!(keyed(&ranked.ranking), reference(&axis(0), &map(&replaced)));
    assert!(!try_flock_exclusive(&fixture.generation_dir(&new_digest)));
    assert!(residency.try_charge(1 << 20).is_none());
    drop(ranked);
    assert!(try_flock_exclusive(&fixture.generation_dir(&new_digest)));
    assert!(residency.try_charge(1 << 20).is_some());

    // A cancelled budget ends the walk without a ranking; the view was still held through the read.
    let view = acquire_view(&mut fixture, &residency, &mut |_| {}).unwrap();
    let budget = EvalBudget::unbounded();
    budget.cancel();
    let worker_store = Arc::clone(&store);
    let outcome = rank_on_worker(
        Arc::clone(&view),
        Arc::clone(&kernel),
        move |walk| {
            worker_store
                .with_conn(|conn| {
                    walk(conn);
                    Ok(())
                })
                .map_err(|error| error.to_string())
        },
        request(),
        budget,
        scratch.clone(),
    )
    .await
    .unwrap();
    assert!(matches!(
        outcome,
        Err(RankRefusal::Layered(
            retrieval::dense::LayeredRefusal::Oracle(
                retrieval::dense::OracleRefusal::BudgetExhausted
            )
        ))
    ));
    assert!(
        !try_flock_exclusive(&fixture.generation_dir(&new_digest)),
        "the caller's view is untouched"
    );
    drop(view);
    assert!(try_flock_exclusive(&fixture.generation_dir(&new_digest)));
}

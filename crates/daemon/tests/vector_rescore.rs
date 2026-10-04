//! RP2.6.U3 witnesses over real lifecycle generations: the quantized pool selected from pinned codes, rescored from the original rows of the same pinned layers, with every read observed.
//! Expected rankings come from the vectors the test wrote, scored by the shared f64 reference; expected pools come from each layer's own scales and codes, scored by the weighted formula restated here.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use daemon::vector_admission::{RESIDENT_LIMIT, ResourceClass};
use daemon::vector_composition::SelectorState;
use daemon::vector_generation::ROWS_FILE;
use daemon::vector_reader::{
    CompressedRanking, CompressedRefusal, CompressedRequest, PinnedVectors, RankRefusal,
    RescoreEvent, rank_compressed,
};
use kernel::applicability::EvalBudget;
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::codec::ARTIFACT_HEADER_BYTES;
use retrieval::dense::export::{ExportedRow, LiveRows};
use retrieval::dense::scalar::{self, Scales};
use retrieval::dense::{
    BLOCK_ROWS, CandidateCapacity, CandidatePolicy, CandidateRefusal, Completion, IncompleteReason,
    LayeredRefusal, OracleRefusal, RowFault, ScanBounds, StorageBounds,
};
use support::dense_projection::{Projection, occurrence_id, reference};
use support::vector_store::{Fixture, KERNEL, unit};

use support::vector_reads::*;

/// A pool of exactly `candidates` for a ranking of `k`: the quotient's binary value can round up past it, so alpha steps down to the largest value whose exact product does not.
fn capacity(k: usize, candidates: usize) -> CandidateCapacity {
    let mut alpha = candidates as f64 / k as f64;
    loop {
        let policy = CandidatePolicy {
            alpha,
            cap: NonZeroUsize::new(candidates).unwrap(),
        };
        if let Ok(Some(capacity)) = CandidateCapacity::new(k, policy) {
            assert_eq!(capacity.candidates().get(), candidates);
            return capacity;
        }
        alpha = alpha.next_down();
    }
}

fn scan_bounds() -> ScanBounds {
    ScanBounds {
        page_rows: NonZeroUsize::new(2).unwrap(),
        max_rows: NonZeroUsize::new(4096).unwrap(),
        storage: StorageBounds {
            batch_bytes: NonZeroUsize::new(1 << 20).unwrap(),
            heap_bytes: NonZeroUsize::new(1 << 20).unwrap(),
        },
    }
}

struct Limits {
    max_layers: usize,
    max_pinned_bytes: u64,
    max_read_bytes: u64,
}

const ROOMY: Limits = Limits {
    max_layers: 8,
    max_pinned_bytes: u64::MAX,
    max_read_bytes: u64::MAX,
};

/// One rank over `view`, recording every rescore event; `at_selection` runs once the pool is selected.
#[allow(clippy::too_many_arguments)]
fn run(
    fixture: &Fixture,
    projection: &Projection,
    view: &PinnedVectors,
    query: &[f32],
    capacity: CandidateCapacity,
    limits: &Limits,
    budget: &EvalBudget,
    mut at_selection: impl FnMut(),
) -> (
    Result<CompressedRanking, CompressedRefusal>,
    Vec<(String, usize)>,
) {
    let expected = fixture.expected();
    let request = CompressedRequest {
        expected: &expected,
        query,
        authority: projection.authority(),
        capacity,
        bounds: scan_bounds(),
        max_entries: NonZeroUsize::new(4096).unwrap(),
        max_layers: NonZeroUsize::new(limits.max_layers).unwrap(),
        max_pinned_bytes: limits.max_pinned_bytes,
        max_read_bytes: limits.max_read_bytes,
    };
    let mut reads = Vec::new();
    let outcome = projection
        .store
        .with_conn(|conn| {
            Ok(rank_compressed(
                view,
                conn,
                &projection.kernel,
                &request,
                budget,
                &fixture.admission,
                &mut |event| match event {
                    RescoreEvent::AfterSelection => at_selection(),
                    RescoreEvent::ReadOriginal { member, row } => {
                        reads.push((member.to_owned(), row));
                    }
                    RescoreEvent::Scan(_) => {}
                },
            ))
        })
        .unwrap();
    (outcome, reads)
}

fn rank_simple(
    fixture: &Fixture,
    projection: &Projection,
    view: &PinnedVectors,
    query: &[f32],
    k: usize,
    candidates: usize,
) -> (CompressedRanking, Vec<(String, usize)>) {
    let (outcome, reads) = run(
        fixture,
        projection,
        view,
        query,
        capacity(k, candidates),
        &ROOMY,
        &EvalBudget::unbounded(),
        || {},
    );
    (outcome.unwrap(), reads)
}

fn weighted(scales: &Scales, query: &[i8], doc: &[i8]) -> f64 {
    let mut total = 0.0f64;
    for index in 0..query.len() {
        let scale = f64::from(scales.as_slice()[index]);
        total += scale * scale * f64::from(i32::from(query[index]) * i32::from(doc[index]));
    }
    total
}

/// The pool an independent reading of the view's layers predicts: each eligible winner `(layer, row)` scored under its own layer, fully sorted, cut at `candidates`.
fn expected_pool(
    view: &PinnedVectors,
    query: &[f32],
    winners: &BTreeMap<String, (usize, usize)>,
    candidates: usize,
) -> Vec<(String, u64)> {
    let mut scored: Vec<(String, f64)> = winners
        .iter()
        .map(|(id, (layer, row))| {
            let layer = &view.layers()[*layer];
            let q = scalar::encode(&view.layout(), &layer.scales, query)
                .unwrap()
                .codes;
            (
                id.clone(),
                weighted(&layer.scales, &q, &layer.codes(*row).unwrap()),
            )
        })
        .collect();
    scored.sort_by(|(a_id, a), (b_id, b)| b.total_cmp(a).then_with(|| a_id.cmp(b_id)));
    scored.truncate(candidates);
    scored
        .into_iter()
        .map(|(id, score)| (id, score.to_bits()))
        .collect()
}

fn winner(view: &PinnedVectors, layer: usize, object: &str) -> (String, (usize, usize)) {
    let id = occurrence_id(object);
    let row = view.layers()[layer]
        .occurrence_ids()
        .iter()
        .position(|candidate| *candidate == id)
        .unwrap();
    (id, (layer, row))
}

fn pool_keys(ranking: &CompressedRanking) -> Vec<(String, u64)> {
    ranking
        .pool
        .ranking
        .ranked
        .iter()
        .map(|row| (row.occurrence_id.clone(), row.score.to_bits()))
        .collect()
}

fn rescored(ranking: &CompressedRanking) -> Vec<(String, f64)> {
    ranking
        .rescored
        .ranked
        .iter()
        .map(|row| (row.occurrence_id.clone(), row.score))
        .collect()
}

/// `Top(K, A, f32)`: the reference over the pool's members only.
fn top_of_pool(
    query: &[f32],
    ranking: &CompressedRanking,
    vectors: &BTreeMap<String, Vec<f32>>,
    k: usize,
) -> Vec<(String, f64)> {
    let pool: Vec<(String, Vec<f32>)> = ranking
        .pool
        .ranking
        .ranked
        .iter()
        .map(|row| {
            (
                row.occurrence_id.clone(),
                vectors[&row.occurrence_id].clone(),
            )
        })
        .collect();
    let mut top = reference(query, &pool);
    top.truncate(k);
    top
}

#[test]
fn only_pool_entries_are_read_and_each_from_its_winning_pinned_layer() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let corpus = corpus();
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    // The delta replaces `gamma` with the axis's best row and tombstones `beta`; the base's `gamma` and `beta` rows are stale.
    let lead = unit([0.95, 0.05, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let delta = fixture.layer_from(&export(&[("gamma", lead.clone())], &["beta"], 12));
    let composition = fixture.compose(1, &base, &[delta]).unwrap();
    fixture.publish(&composition).unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let query = axis(0);

    let (ranking, reads) = rank_simple(&fixture, &projection, &view, &query, 2, 3);
    let winners: BTreeMap<String, (usize, usize)> = [
        winner(&view, 0, "alpha"),
        winner(&view, 1, "gamma"),
        winner(&view, 0, "delta"),
        winner(&view, 0, "epsilon"),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        pool_keys(&ranking),
        expected_pool(&view, &query, &winners, 3)
    );
    // `beta` is live in the projection and masked: a shortfall that keeps the pool.
    assert_eq!(
        ranking.pool.ranking.completion,
        Completion::Incomplete(IncompleteReason::DenseCoverageShortfall)
    );

    // Exactly one read per pool entry, from the member and row its winner names.
    let expected_reads: Vec<(String, usize)> = ranking
        .pool
        .winners
        .iter()
        .map(|winner| (view.layers()[winner.layer].digest.clone(), winner.row))
        .collect();
    assert_eq!(reads, expected_reads);
    let stale_gamma = winner(&view, 0, "gamma").1;
    let stale_beta = winner(&view, 0, "beta").1;
    for stale in [stale_gamma, stale_beta] {
        assert!(!reads.contains(&(view.layers()[0].digest.clone(), stale.1)));
    }

    let mut vectors: BTreeMap<String, Vec<f32>> = map(&corpus).into_iter().collect();
    vectors.insert(occurrence_id("gamma"), lead);
    assert_eq!(
        rescored(&ranking),
        top_of_pool(&query, &ranking, &vectors, 2)
    );
    assert_eq!(
        ranking.rescored.ranked[0].occurrence_id,
        occurrence_id("gamma")
    );
    for (row, candidate) in ranking
        .rescored
        .ranked
        .iter()
        .zip(&ranking.rescored.candidates)
    {
        assert_eq!(row.occurrence_id, candidate.occurrence_id);
    }
    assert_eq!(held(&fixture.ledger, ResourceClass::Scratch), 0);
    assert_eq!(held(&fixture.ledger, ResourceClass::RowBuffers), 0);
}

#[test]
fn negative_scores_ties_and_an_underfilled_pool_keep_the_global_order() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let mut corpus = corpus();
    // `delta` and `epsilon` carry the same row, so only identifier bytes order them.
    corpus[4].1 = corpus[3].1.clone();
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let negative = negative_axis();
    let vectors: BTreeMap<String, Vec<f32>> = map(&corpus).into_iter().collect();
    for query in [axis(0), negative, axis(4)] {
        let (ranking, reads) = rank_simple(&fixture, &projection, &view, &query, 5, 8);
        assert_eq!(
            ranking.pool.ranking.ranked.len(),
            5,
            "the pool holds E and no more"
        );
        assert_eq!(reads.len(), 5);
        assert_eq!(rescored(&ranking), reference(&query, &map(&corpus)));
        assert_eq!(
            rescored(&ranking),
            top_of_pool(&query, &ranking, &vectors, 5)
        );
    }
    let (ranking, _) = rank_simple(&fixture, &projection, &view, &negative_axis(), 5, 8);
    assert!(ranking.rescored.ranked.iter().all(|row| row.score <= 0.0));
    let ids: Vec<String> = rescored(&ranking).into_iter().map(|(id, _)| id).collect();
    let (d, e) = (occurrence_id("delta"), occurrence_id("epsilon"));
    let (first, second) = if d < e { (d, e) } else { (e, d) };
    let at = |id: &str| ids.iter().position(|other| other == id).unwrap();
    assert_eq!(
        at(&second),
        at(&first) + 1,
        "equal rows stay two entries, smaller bytes first"
    );
}

fn negative_axis() -> Vec<f32> {
    let mut query = axis(0);
    query[0] = -1.0;
    query
}

#[test]
fn the_ranking_is_the_same_however_the_rows_are_spread_across_layers() {
    let lead = unit([0.95, 0.05, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let mut finals: Vec<Vec<(String, u64)>> = Vec::new();
    for spread in 0..3 {
        let mut fixture = Fixture::new();
        let projection = projection(&fixture, &OBJECTS);
        let corpus = corpus();
        let mut flat: Vec<(&str, Vec<f32>)> = corpus
            .iter()
            .filter(|(object, _)| *object != "beta")
            .cloned()
            .collect();
        for row in &mut flat {
            if row.0 == "gamma" {
                row.1 = lead.clone();
            }
        }
        let composition = match spread {
            // One base holding the resolved rows.
            0 => {
                let base = fixture.layer_from(&export(&flat, &[], 10));
                fixture.compose(1, &base, &[]).unwrap()
            }
            // The stale rows below one delta.
            1 => {
                let base = fixture.layer_from(&export(&corpus, &[], 10));
                let delta = fixture.layer_from(&export(&[("gamma", lead.clone())], &["beta"], 12));
                fixture.compose(1, &base, &[delta]).unwrap()
            }
            // Two deltas, the later superseding the earlier's own `gamma`.
            _ => {
                let base = fixture.layer_from(&export(&corpus, &[], 10));
                let first = fixture.layer_from(&export(&[("gamma", axis(7))], &[], 12));
                let second = fixture.layer_from(&export(&[("gamma", lead.clone())], &["beta"], 14));
                fixture.compose(1, &base, &[first, second]).unwrap()
            }
        };
        fixture.publish(&composition).unwrap();
        let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
        let (ranking, reads) = rank_simple(&fixture, &projection, &view, &axis(0), 3, 4);
        assert_eq!(reads.len(), 4, "spread {spread}");
        finals.push(
            ranking
                .rescored
                .ranked
                .iter()
                .map(|row| (row.occurrence_id.clone(), row.score.to_bits()))
                .collect(),
        );
    }
    assert_eq!(finals[0], finals[1]);
    assert_eq!(finals[1], finals[2]);
}

#[test]
fn a_promotion_and_prune_after_selection_leave_the_rescore_on_the_pinned_files() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let corpus = corpus();
    let old_base = fixture.layer_from(&export(&corpus, &[], 10));
    fixture
        .publish(&fixture.compose(1, &old_base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let query = axis(0);
    let mut new_digest = None;
    let (outcome, reads) = run(
        &fixture,
        &projection,
        &view,
        &query,
        capacity(3, 4),
        &ROOMY,
        &EvalBudget::unbounded(),
        || {
            let (_, new_base) = publish_replacement(&fixture, &corpus, 2);
            let report = fixture.store.prune(&BTreeSet::new()).unwrap();
            assert_eq!(report.removed_generations, 0, "the reader pins the old set");
            new_digest = Some(new_base.digest);
        },
    );
    let ranking = outcome.unwrap();
    let new_digest = new_digest.unwrap();
    assert!(reads.iter().all(|(member, _)| *member == old_base.digest));
    assert!(!reads.iter().any(|(member, _)| *member == new_digest));
    let vectors: BTreeMap<String, Vec<f32>> = map(&corpus).into_iter().collect();
    assert_eq!(
        rescored(&ranking),
        top_of_pool(&query, &ranking, &vectors, 3)
    );
    assert_eq!(
        ranking.rescored.ranked[0].occurrence_id,
        occurrence_id("alpha")
    );
}

#[test]
fn a_missing_accepted_row_quarantines_the_view_and_recovery_serves_the_prior_set_under_current_eligibility()
 {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let corpus = corpus();
    let old_base = fixture.layer_from(&export(&corpus, &[], 10));
    let old_digest = fixture
        .publish(&fixture.compose(1, &old_base, &[]).unwrap())
        .unwrap();
    let (replaced, new_base) = publish_replacement(&fixture, &corpus, 2);
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    assert_eq!(view.members(), vec![new_base.digest.clone()]);
    let rows = fixture.generation_dir(&new_base.digest).join(ROWS_FILE);
    let query = axis(0);

    // The rows are cut to the header after the pool is selected: every accepted row is missing.
    let (outcome, reads) = run(
        &fixture,
        &projection,
        &view,
        &query,
        capacity(2, 3),
        &ROOMY,
        &EvalBudget::unbounded(),
        || {
            let bytes = std::fs::read(&rows).unwrap();
            std::fs::write(&rows, &bytes[..ARTIFACT_HEADER_BYTES]).unwrap();
        },
    );
    match outcome {
        Err(CompressedRefusal::Corrupt { member, fault, .. }) => {
            assert_eq!(member, new_base.digest);
            assert!(matches!(fault, RowFault::Missing(_)), "{fault:?}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(reads.len(), 1, "the first missing row ends the request");
    assert!(view.is_quarantined());
    assert_eq!(held(&fixture.ledger, ResourceClass::Scratch), 0);
    assert_eq!(held(&fixture.ledger, ResourceClass::RowBuffers), 0);
    let (again, reads) = run(
        &fixture,
        &projection,
        &view,
        &query,
        capacity(2, 3),
        &ROOMY,
        &EvalBudget::unbounded(),
        || panic!("a quarantined view selects nothing"),
    );
    assert_eq!(
        again.unwrap_err(),
        CompressedRefusal::Quarantined {
            digest: view.digest().to_owned()
        }
    );
    assert!(reads.is_empty());
    assert!(matches!(
        rank_view(&fixture, &projection, &view, &query, 2).unwrap_err(),
        RankRefusal::Quarantined { .. }
    ));
    drop(view);

    // A new acquisition re-verifies: the cut member fails, and recovery takes the prior verified composition.
    let (recovered, selector) = fixture.recover().unwrap();
    assert_eq!(recovered, old_digest);
    assert!(matches!(selector, SelectorState::Stale(_)));
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    assert_eq!(view.digest(), old_digest);
    // Current canonical eligibility still applies to the older rows.
    projection.retire("alpha");
    let (ranking, reads) = rank_simple(&fixture, &projection, &view, &query, 2, 3);
    assert!(reads.iter().all(|(member, _)| *member == old_base.digest));
    assert!(
        !rescored(&ranking)
            .iter()
            .any(|(id, _)| *id == occurrence_id("alpha"))
    );
    let mut eligible = map(&corpus);
    eligible.retain(|(id, _)| *id != occurrence_id("alpha"));
    let mut expected = reference(&query, &eligible);
    expected.truncate(2);
    assert_eq!(rescored(&ranking), expected);
    let _ = replaced;
}

/// Rewrites every original row of `digest` in place after selection; the bytes stay where the layer declares them.
fn rewrite_rows(fixture: &Fixture, digest: &str, rewrite: fn(&mut [u8])) -> impl FnMut() {
    let rows = fixture.generation_dir(digest).join(ROWS_FILE);
    move || {
        let mut bytes = std::fs::read(&rows).unwrap();
        let width = 8 * 4;
        for row in bytes[ARTIFACT_HEADER_BYTES..].chunks_mut(width) {
            rewrite(row);
        }
        std::fs::write(&rows, bytes).unwrap();
    }
}

#[test]
fn a_corrupt_accepted_row_is_refused_and_quarantined_without_a_substitute() {
    // A NaN coordinate fails the row's own decode; a doubled row is finite but leaves the unit-norm tolerance, which only the rescore's layout check sees.
    let nan = |row: &mut [u8]| row[..4].copy_from_slice(&f32::NAN.to_le_bytes());
    let doubled = |row: &mut [u8]| {
        for word in row.chunks_mut(4) {
            let value = f32::from_le_bytes(word.try_into().unwrap()) * 2.0;
            word.copy_from_slice(&value.to_le_bytes());
        }
    };
    let rewrites: [fn(&mut [u8]); 2] = [nan, doubled];
    for rewrite in rewrites {
        let mut fixture = Fixture::new();
        let projection = projection(&fixture, &OBJECTS);
        let base = fixture.layer_from(&export(&corpus(), &[], 10));
        fixture
            .publish(&fixture.compose(1, &base, &[]).unwrap())
            .unwrap();
        let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
        let (outcome, reads) = run(
            &fixture,
            &projection,
            &view,
            &axis(0),
            capacity(5, 5),
            &ROOMY,
            &EvalBudget::unbounded(),
            rewrite_rows(&fixture, &base.digest, rewrite),
        );
        let first = occurrence_id("alpha");
        match outcome {
            Err(CompressedRefusal::Corrupt {
                member,
                occurrence_id,
                fault: RowFault::Rejected(_),
            }) => {
                assert_eq!(member, base.digest);
                assert_eq!(occurrence_id, first, "the pool's best entry is read first");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(reads.len(), 1);
        assert!(view.is_quarantined());
    }
}

#[test]
fn missing_codes_found_by_the_scan_quarantine_the_view_and_ordinary_refusals_do_not() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();

    // A cancelled scan refuses without touching the view.
    let cancelled = EvalBudget::unbounded();
    cancelled.cancel();
    let (outcome, _) = run(
        &fixture,
        &projection,
        &view,
        &axis(0),
        capacity(2, 3),
        &ROOMY,
        &cancelled,
        || panic!("a cancelled scan selects nothing"),
    );
    assert!(
        matches!(outcome, Err(CompressedRefusal::Candidates(_))),
        "{outcome:?}"
    );
    assert!(!view.is_quarantined());

    // The codes file is cut to nothing: the first winner's codes are missing.
    let codes = fixture
        .generation_dir(&base.digest)
        .join(daemon::vector_generation::CODES_FILE);
    std::fs::write(&codes, []).unwrap();
    let (outcome, reads) = run(
        &fixture,
        &projection,
        &view,
        &axis(0),
        capacity(2, 3),
        &ROOMY,
        &EvalBudget::unbounded(),
        || panic!("nothing is selected"),
    );
    assert!(
        matches!(
            outcome,
            Err(CompressedRefusal::Candidates(CandidateRefusal::Layered(
                LayeredRefusal::Oracle(OracleRefusal::Unreadable { .. })
            )))
        ),
        "{outcome:?}"
    );
    assert!(reads.is_empty());
    assert!(view.is_quarantined());
}

#[test]
fn a_code_window_across_a_cut_serves_the_rows_before_it_and_refuses_the_first_missing_row() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    // Two of the five rows of codes remain, so the layer's first window cannot be read whole.
    let codes = fixture
        .generation_dir(&base.digest)
        .join(daemon::vector_generation::CODES_FILE);
    std::fs::File::options()
        .write(true)
        .open(&codes)
        .unwrap()
        .set_len(2 * 8)
        .unwrap();
    let (outcome, reads) = run(
        &fixture,
        &projection,
        &view,
        &axis(0),
        capacity(2, 3),
        &ROOMY,
        &EvalBudget::unbounded(),
        || panic!("nothing is selected"),
    );
    match outcome {
        Err(CompressedRefusal::Candidates(CandidateRefusal::Layered(LayeredRefusal::Oracle(
            OracleRefusal::Unreadable { occurrence_id, .. },
        )))) => assert_eq!(occurrence_id, view.layers()[0].occurrence_ids()[2]),
        other => panic!("{other:?}"),
    }
    assert!(reads.is_empty());
    assert!(view.is_quarantined());
    assert_eq!(held(&fixture.ledger, ResourceClass::Scratch), 0);
}

/// Unit rows `dimension` wide from a linear congruential sequence; only their order under each layer's scales matters.
fn wide_rows(names: &[String], dimension: usize, seed: u32) -> Vec<(String, Vec<f32>)> {
    let mut state = seed;
    names
        .iter()
        .map(|name| {
            let raw: Vec<f64> = (0..dimension)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    f64::from(state >> 8) / f64::from(1u32 << 24) - 0.5
                })
                .collect();
            let norm = raw.iter().map(|value| value * value).sum::<f64>().sqrt();
            let row = raw.iter().map(|value| (value / norm) as f32).collect();
            (name.clone(), row)
        })
        .collect()
}

fn wide_export(fixture: &Fixture, rows: &[(String, Vec<f32>)], checkpoint: i64) -> LiveRows {
    let mut rows: Vec<ExportedRow> = rows
        .iter()
        .map(|(object, vector)| ExportedRow {
            occurrence_id: occurrence_id(object),
            vector: vector.clone(),
        })
        .collect();
    rows.sort_by(|a, b| a.occurrence_id.cmp(&b.occurrence_id));
    LiveRows {
        generation: fixture.generation.clone(),
        kernel_incarnation_id: KERNEL.to_owned(),
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: checkpoint - 1,
            checkpoint_commit_seq: checkpoint,
            hold_id: "hold-7".to_owned(),
        },
        rows,
        tombstones: Vec::new(),
    }
}

#[test]
fn a_layer_spanning_several_code_windows_selects_the_pool_its_own_codes_predict() {
    // Four rows of 4096 codes fill one window, so the base's eleven rows span three windows and the delta's winners interleave with them.
    const WIDE: u32 = 4096;
    let names: Vec<String> = (0..11).map(|index| format!("wide-{index:02}")).collect();
    let objects: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut fixture = Fixture::with_dimension(WIDE);
    let projection = Projection::new(
        fixture.root.path(),
        &fixture.identity,
        &fixture.generation,
        &objects,
        &objects,
    );
    let rows = wide_rows(&names, WIDE as usize, 0x2545_f491);
    let replaced: Vec<String> = vec![names[1].clone(), names[6].clone()];
    let delta_rows = wide_rows(&replaced, WIDE as usize, 0x0bad_5eed);
    let base = fixture.layer_from(&wide_export(&fixture, &rows, 10));
    let delta = fixture.layer_from(&wide_export(&fixture, &delta_rows, 12));
    fixture
        .publish(&fixture.compose(1, &base, &[delta]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let query = wide_rows(&["query".to_owned()], WIDE as usize, 0x1234_5678)
        .pop()
        .unwrap()
        .1;
    let winners: BTreeMap<String, (usize, usize)> = names
        .iter()
        .map(|name| {
            let layer = usize::from(replaced.contains(name));
            winner(&view, layer, name)
        })
        .collect();
    let (ranking, reads) = rank_simple(&fixture, &projection, &view, &query, 3, 8);
    assert_eq!(
        pool_keys(&ranking),
        expected_pool(&view, &query, &winners, 8)
    );
    assert_eq!(reads.len(), 8);
    let mut vectors: BTreeMap<String, Vec<f32>> = rows
        .iter()
        .map(|(name, row)| (occurrence_id(name), row.clone()))
        .collect();
    for (name, row) in &delta_rows {
        vectors.insert(occurrence_id(name), row.clone());
    }
    assert_eq!(
        rescored(&ranking),
        top_of_pool(&query, &ranking, &vectors, 3)
    );
    assert_eq!(held(&fixture.ledger, ResourceClass::Scratch), 0);
}

#[test]
fn a_view_of_another_identity_is_refused_first() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let mut expected = fixture.expected();
    expected.kernel_incarnation_id = "another-kernel";
    let query = axis(0);
    let request = CompressedRequest {
        expected: &expected,
        query: &query,
        authority: projection.authority(),
        capacity: capacity(2, 3),
        bounds: scan_bounds(),
        max_entries: NonZeroUsize::new(64).unwrap(),
        max_layers: NonZeroUsize::new(8).unwrap(),
        max_pinned_bytes: u64::MAX,
        max_read_bytes: u64::MAX,
    };
    let outcome = projection
        .store
        .with_conn(|conn| {
            Ok(rank_compressed(
                &view,
                conn,
                &projection.kernel,
                &request,
                &EvalBudget::unbounded(),
                &fixture.admission,
                &mut |_| panic!("nothing is selected"),
            ))
        })
        .unwrap();
    assert!(
        matches!(outcome, Err(CompressedRefusal::Identity { .. })),
        "{outcome:?}"
    );
}

#[test]
fn cancellation_during_the_rescore_is_a_budget_refusal_and_quarantines_nothing() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let budget = EvalBudget::unbounded();
    let (outcome, reads) = run(
        &fixture,
        &projection,
        &view,
        &axis(0),
        capacity(2, 3),
        &ROOMY,
        &budget,
        || budget.cancel(),
    );
    assert_eq!(outcome.unwrap_err(), CompressedRefusal::Budget);
    assert!(reads.is_empty());
    assert!(!view.is_quarantined());
    assert_eq!(held(&fixture.ledger, ResourceClass::RowBuffers), 0);
}

#[test]
fn every_view_and_read_bound_refuses_before_the_projection_is_read() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let corpus = corpus();
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    let delta = fixture.layer_from(&export(&[("alpha", axis(7))], &[], 12));
    fixture
        .publish(&fixture.compose(1, &base, &[delta]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let query = axis(0);
    let row = 8 * 4;
    let attempt = |limits: Limits| {
        run(
            &fixture,
            &projection,
            &view,
            &query,
            capacity(2, 3),
            &limits,
            &EvalBudget::unbounded(),
            || panic!("nothing is selected"),
        )
    };
    let (refused, reads) = attempt(Limits {
        max_read_bytes: 3 * row - 1,
        ..ROOMY
    });
    assert_eq!(
        refused.unwrap_err(),
        CompressedRefusal::ReadBytes {
            bytes: 3 * row,
            limit: 3 * row - 1
        }
    );
    assert!(reads.is_empty());
    let (exact, reads) = run(
        &fixture,
        &projection,
        &view,
        &query,
        capacity(2, 3),
        &Limits {
            max_read_bytes: 3 * row,
            ..ROOMY
        },
        &EvalBudget::unbounded(),
        || {},
    );
    assert!(exact.is_ok(), "the exact read bound admits a full pool");
    assert_eq!(reads.len(), 3);
    assert_eq!(
        attempt(Limits {
            max_layers: 1,
            ..ROOMY
        })
        .0
        .unwrap_err(),
        CompressedRefusal::Layers {
            layers: 2,
            limit: 1
        }
    );
    let pinned = view.pinned_bytes();
    assert!(pinned > 0);
    assert_eq!(
        attempt(Limits {
            max_pinned_bytes: pinned - 1,
            ..ROOMY
        })
        .0
        .unwrap_err(),
        CompressedRefusal::PinnedBytes {
            bytes: pinned,
            limit: pinned - 1
        }
    );
    // A resident limit the view's tables already fill leaves no room for the pool's row buffers.
    let tables = fixture.ledger.census().resident;
    // One block of decoded codes and one window per layer; a window this narrow holds every row of its layer.
    let windows: usize = view
        .layers()
        .iter()
        .map(|layer| layer.occurrence_ids().len())
        .sum();
    let code_scratch = (BLOCK_ROWS + windows) as u64 * 8;
    fixture.set_limit(RESIDENT_LIMIT, tables + code_scratch);
    let (refused, _) = run(
        &fixture,
        &projection,
        &view,
        &query,
        capacity(2, 3),
        &ROOMY,
        &EvalBudget::unbounded(),
        || panic!("nothing is selected"),
    );
    assert!(matches!(
        refused,
        Err(CompressedRefusal::Reservation {
            class: ResourceClass::RowBuffers,
            ..
        })
    ));
    assert_eq!(held(&fixture.ledger, ResourceClass::Scratch), 0);
}

/// One stage record for one alpha: the f32 baseline, the quantized pool, and the rescored ranking, each with its own identities and its own measure.
#[derive(Debug)]
struct StageEvidence {
    alpha: usize,
    baseline: Vec<String>,
    candidates: Vec<String>,
    rescored: Vec<String>,
    /// `|A ∩ T| / |T|`; `None` when the eligible baseline is empty.
    candidate_coverage: Option<f64>,
    /// `|rescored ∩ T| / |T|`; `None` when the eligible baseline is empty.
    rescored_recall_at_10: Option<f64>,
}

fn fraction(found: &[String], baseline: &[String]) -> Option<f64> {
    (!baseline.is_empty()).then(|| {
        found.iter().filter(|id| baseline.contains(id)).count() as f64 / baseline.len() as f64
    })
}

/// The sweep's query: a fixed unit direction the corpus clusters around.
const CENTER: [f32; 8] = [0.6, -0.3, 0.4, 0.2, -0.1, 0.5, 0.25, -0.15];

/// A fixed corpus of sixty unit rows: the center plus a small offset drawn from a linear congruential sequence, so neighbors sit closer than the int8 step and quantization reorders them.
fn spread_corpus(objects: &[String]) -> Vec<(String, Vec<f32>)> {
    let mut state: u32 = 0x2545_f491;
    objects
        .iter()
        .map(|object| {
            let raw: [f32; 8] = std::array::from_fn(|coordinate| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let offset = (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                CENTER[coordinate] + 0.05 * offset
            });
            (object.clone(), unit(raw))
        })
        .collect()
}

#[test]
fn the_alpha_sweep_keeps_candidate_coverage_and_rescored_recall_as_separate_stage_records() {
    let names: Vec<String> = (0..60).map(|index| format!("obj-{index:02}")).collect();
    let objects: Vec<&str> = names.iter().map(String::as_str).collect();
    // Every fifth object is unadmitted, so the eligible baseline is a strict subset.
    let admitted: Vec<&str> = objects
        .iter()
        .enumerate()
        .filter(|(index, _)| index % 5 != 0)
        .map(|(_, object)| *object)
        .collect();
    let mut fixture = Fixture::new();
    let projection = Projection::new(
        fixture.root.path(),
        &fixture.identity,
        &fixture.generation,
        &objects,
        &admitted,
    );
    let rows = spread_corpus(&names);
    let refs: Vec<(&str, Vec<f32>)> = rows.iter().map(|(o, v)| (o.as_str(), v.clone())).collect();
    let base = fixture.layer_from(&export(&refs, &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let query = unit(CENTER);
    let eligible: Vec<(String, Vec<f32>)> = rows
        .iter()
        .filter(|(object, _)| admitted.contains(&object.as_str()))
        .map(|(object, vector)| (occurrence_id(object), vector.clone()))
        .collect();
    let mut baseline: Vec<String> = reference(&query, &eligible)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    baseline.truncate(10);
    let mut records = Vec::new();
    for alpha in [1usize, 2, 5, 10, 20, 50] {
        let candidates = (10 * alpha).min(1024);
        let (ranking, reads) = rank_simple(&fixture, &projection, &view, &query, 10, candidates);
        assert_eq!(reads.len(), ranking.pool.ranking.ranked.len());
        let candidate_ids: Vec<String> = ranking
            .pool
            .ranking
            .ranked
            .iter()
            .map(|row| row.occurrence_id.clone())
            .collect();
        let rescored_ids: Vec<String> = ranking
            .rescored
            .ranked
            .iter()
            .map(|row| row.occurrence_id.clone())
            .collect();
        records.push(StageEvidence {
            alpha,
            candidate_coverage: fraction(&candidate_ids, &baseline),
            rescored_recall_at_10: fraction(&rescored_ids, &baseline),
            baseline: baseline.clone(),
            candidates: candidate_ids,
            rescored: rescored_ids,
        });
    }
    for record in &records {
        assert_eq!(record.baseline.len(), 10, "{record:?}");
        assert!(record.candidates.len() >= record.rescored.len());
        let coverage = record.candidate_coverage.unwrap();
        let recall = record.rescored_recall_at_10.unwrap();
        // Exact rescore keeps every baseline member the pool holds, within K: the two may be equal and are still separate fields.
        assert!(recall <= coverage + f64::EPSILON, "{record:?}");
    }
    for pair in records.windows(2) {
        assert!(pair[1].candidate_coverage >= pair[0].candidate_coverage);
    }
    let widest = records.last().unwrap();
    assert_eq!(
        widest.candidate_coverage,
        Some(1.0),
        "a pool over all of E covers it"
    );
    assert_eq!(
        widest.rescored, widest.baseline,
        "and rescore then equals the oracle"
    );
    assert_eq!(
        records
            .iter()
            .map(|record| record.alpha)
            .collect::<Vec<_>>(),
        [1, 2, 5, 10, 20, 50]
    );
    // The stages differ where the pool is narrow: quantization drops part of the baseline at alpha one.
    assert!(
        records[0].candidate_coverage < Some(1.0),
        "{:?}",
        records[0]
    );
    // The stage helper gives an empty baseline no value rather than a perfect score.
    assert_eq!(fraction(&[], &[]), None);
}

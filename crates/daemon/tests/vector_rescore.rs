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
    let activation = fixture.activation();

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
        activation: &activation,
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
fn an_ordinary_read_failure_of_an_accepted_row_is_io_and_leaves_the_view_usable() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let rank = || {
        run(
            &fixture,
            &projection,
            &view,
            &axis(0),
            capacity(5, 5),
            &ROOMY,
            &EvalBudget::unbounded(),
            || {},
        )
    };
    let (healthy, _) = rank();
    let healthy = healthy.unwrap();

    // An error other than a file ending early is a failed read: the request refuses with the accepted row named after attempting only that row, and the view stays usable.
    for kind in [
        std::io::ErrorKind::Other,
        std::io::ErrorKind::PermissionDenied,
    ] {
        view.fail_original_reads_for_test(Some(kind));
        let (outcome, reads) = rank();
        match outcome {
            Err(CompressedRefusal::Io {
                member,
                occurrence_id: refused,
                ..
            }) => {
                assert_eq!(member, base.digest, "{kind:?}");
                assert_eq!(refused, occurrence_id("alpha"), "{kind:?}");
            }
            other => panic!("{kind:?}: {other:?}"),
        }
        assert_eq!(reads.len(), 1, "{kind:?}: the first accepted row only");
        assert!(!view.is_quarantined(), "{kind:?}");
    }

    view.fail_original_reads_for_test(None);
    let (recovered, _) = rank();
    let recovered = recovered.unwrap();
    assert_eq!(
        recovered.rescored, healthy.rescored,
        "the same view ranks again once reads succeed"
    );

    // The same seam reporting a file that ends early is a missing row, which quarantines.
    view.fail_original_reads_for_test(Some(std::io::ErrorKind::UnexpectedEof));
    let (outcome, _) = rank();
    assert!(
        matches!(
            outcome,
            Err(CompressedRefusal::Corrupt {
                fault: RowFault::Missing(_),
                ..
            })
        ),
        "{outcome:?}"
    );
    assert!(view.is_quarantined());
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
fn a_code_file_cut_before_the_scan_serves_the_rows_before_it_and_refuses_the_first_missing_row() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    // Two of the five rows of codes remain when the scan maps the file, so only those two are mapped.
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
    // Four rows of 16384 codes fill one window, so the base's eleven rows span three windows and the delta's winners interleave with them.
    const WIDE: u32 = 16384;
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
    let activation = fixture.activation();

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
        activation: &activation,
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

/// A pool with no entry reads no original row, so the rescore alone would never consult the budget; the request still ends as `Budget`.
#[test]
fn cancellation_after_an_empty_selection_is_a_budget_refusal() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    for object in OBJECTS {
        projection.retire(object);
    }
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
}

#[test]
fn a_corrupt_row_found_by_the_f32_ranking_quarantines_the_view() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    rewrite_rows(&fixture, &base.digest, |row| {
        row[..4].copy_from_slice(&f32::NAN.to_le_bytes());
    })();
    let outcome = rank_view(&fixture, &projection, &view, &axis(0), 2);
    assert!(
        matches!(
            outcome,
            Err(RankRefusal::Layered(LayeredRefusal::Oracle(
                OracleRefusal::StoredRow { .. }
            )))
        ),
        "{outcome:?}"
    );
    assert!(view.is_quarantined());
    let (compressed, reads) = run(
        &fixture,
        &projection,
        &view,
        &axis(0),
        capacity(2, 3),
        &ROOMY,
        &EvalBudget::unbounded(),
        || {},
    );
    assert!(matches!(
        compressed,
        Err(CompressedRefusal::Quarantined { .. })
    ));
    assert!(reads.is_empty());
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
    // One block of decoded codes, one encoded query, and one window per layer; a window this narrow holds every row of its layer.
    let windows: usize = view
        .layers()
        .iter()
        .map(|layer| layer.occurrence_ids().len())
        .sum();
    let code_scratch = ((BLOCK_ROWS + 1) * view.layers().len() + windows) as u64 * 8;
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

/// The budget is read after the read observer runs, so a cancellation delivered at an original read ends the request before that row's bytes are read.
#[test]
fn cancellation_at_an_original_read_ends_the_request_before_the_row_is_read() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let budget = EvalBudget::unbounded();
    let expected = fixture.expected();
    let activation = fixture.activation();

    let request = CompressedRequest {
        expected: &expected,
        query: &axis(0),
        authority: projection.authority(),
        capacity: capacity(2, 3),
        bounds: scan_bounds(),
        max_entries: NonZeroUsize::new(4096).unwrap(),
        max_layers: NonZeroUsize::new(ROOMY.max_layers).unwrap(),
        max_pinned_bytes: ROOMY.max_pinned_bytes,
        max_read_bytes: ROOMY.max_read_bytes,
        activation: &activation,
    };
    // A row rewritten to NaN fails its decode, so a read that happens after the cancellation shows as corruption and a quarantine.
    let mut poison = rewrite_rows(&fixture, &base.digest, |row| {
        row[..4].copy_from_slice(&f32::NAN.to_le_bytes());
    });
    let mut reads = 0usize;
    let outcome = projection
        .store
        .with_conn(|conn| {
            Ok(rank_compressed(
                &view,
                conn,
                &projection.kernel,
                &request,
                &budget,
                &fixture.admission,
                &mut |event| {
                    if matches!(event, RescoreEvent::ReadOriginal { .. }) {
                        reads += 1;
                        budget.cancel();
                        poison();
                    }
                },
            ))
        })
        .unwrap();
    assert_eq!(outcome.unwrap_err(), CompressedRefusal::Budget);
    assert_eq!(
        reads, 1,
        "the first original read is observed, then the request ends"
    );
    assert!(!view.is_quarantined(), "the poisoned row was never read");
    assert_eq!(held(&fixture.ledger, ResourceClass::RowBuffers), 0);
}

/// The scan's `Scratch` reservation covers the payload buffers the scan retains per layer: one block of decoded codes, the layer's mapped codes, and the query encoded under that layer's scales.
#[test]
fn the_scan_scratch_charges_the_encoded_query_of_every_layer() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    let first = fixture.layer_from(&export(&[("gamma", axis(7))], &[], 12));
    let second = fixture.layer_from(&export(&[("gamma", axis(1))], &["beta"], 14));
    fixture
        .publish(&fixture.compose(1, &base, &[first, second]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let layers = view.layers().len();
    assert_eq!(layers, 3);
    let dimension = 8u64;
    let mapped: u64 = view
        .layers()
        .iter()
        .map(|layer| layer.occurrence_ids().len() as u64)
        .sum();
    let expected = (BLOCK_ROWS as u64 * layers as u64 + mapped + layers as u64) * dimension;
    let at_selection = std::cell::Cell::new(None);
    let (outcome, _) = run(
        &fixture,
        &projection,
        &view,
        &axis(0),
        capacity(2, 3),
        &ROOMY,
        &EvalBudget::unbounded(),
        || at_selection.set(Some(held(&fixture.ledger, ResourceClass::Scratch))),
    );
    outcome.unwrap();
    assert_eq!(
        at_selection.get(),
        Some(expected),
        "scratch held through the scan: {BLOCK_ROWS} decoded rows, the mapped codes, and the encoded query per layer"
    );
    assert_eq!(held(&fixture.ledger, ResourceClass::Scratch), 0);
}

/// A cancellation delivered at the walk's final `AfterPage` ends the scan before any mapped code is read for scoring.
#[test]
fn cancellation_at_the_final_page_ends_the_scan_before_the_codes_are_scored() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let base = fixture.layer_from(&export(&corpus(), &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let budget = EvalBudget::unbounded();
    let expected = fixture.expected();
    // One page holds the whole population, so its `AfterPage` is the final one.
    let mut bounds = scan_bounds();
    bounds.page_rows = NonZeroUsize::new(64).unwrap();
    let activation = fixture.activation();

    let request = CompressedRequest {
        expected: &expected,
        query: &axis(0),
        authority: projection.authority(),
        capacity: capacity(2, 3),
        bounds,
        max_entries: NonZeroUsize::new(4096).unwrap(),
        max_layers: NonZeroUsize::new(ROOMY.max_layers).unwrap(),
        max_pinned_bytes: ROOMY.max_pinned_bytes,
        max_read_bytes: ROOMY.max_read_bytes,
        activation: &activation,
    };
    // A codes file cut to nothing makes any scoring read visible as a missing-codes refusal and a quarantine.
    let codes = fixture
        .generation_dir(&base.digest)
        .join(daemon::vector_generation::CODES_FILE);
    let mut pages = 0usize;
    let outcome = projection
        .store
        .with_conn(|conn| {
            Ok(rank_compressed(
                &view,
                conn,
                &projection.kernel,
                &request,
                &budget,
                &fixture.admission,
                &mut |event| {
                    if let RescoreEvent::Scan(retrieval::dense::Window::AfterPage(_)) = event {
                        pages += 1;
                        budget.cancel();
                        std::fs::write(&codes, []).unwrap();
                    }
                },
            ))
        })
        .unwrap();
    assert_eq!(pages, 1, "the population fits one page");
    assert!(
        matches!(
            outcome,
            Err(CompressedRefusal::Budget | CompressedRefusal::Candidates(_))
        ),
        "{outcome:?}"
    );
    assert!(
        !matches!(
            outcome,
            Err(CompressedRefusal::Candidates(CandidateRefusal::Layered(
                LayeredRefusal::Oracle(OracleRefusal::Unreadable { .. })
            )))
        ),
        "the cut codes were never read: {outcome:?}"
    );
    assert!(!view.is_quarantined(), "the cancelled scan read no codes");
    assert_eq!(held(&fixture.ledger, ResourceClass::Scratch), 0);
}

/// The base's `gamma` and `beta` rows are the query's best rows, so any selection that admitted them would rank them first. The delta supersedes `gamma` with a poor row and tombstones `beta`: the pool holds exactly the resolved winners, the stale rows outrank every member under the base's own codes, and neither stale row gets an original-f32 read.
#[test]
fn stale_and_tombstoned_rows_that_would_score_highest_never_enter_the_pool() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let mut corpus = corpus();
    for (object, vector) in &mut corpus {
        if matches!(*object, "gamma" | "beta") {
            *vector = unit([0.99, 0.01, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        }
    }
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    let poor = axis(3);
    let delta = fixture.layer_from(&export(&[("gamma", poor.clone())], &["beta"], 12));
    fixture
        .publish(&fixture.compose(1, &base, &[delta]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let query = axis(0);

    let (ranking, reads) = rank_simple(&fixture, &projection, &view, &query, 4, 4);
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
        expected_pool(&view, &query, &winners, 4)
    );
    // Under the base's own scales, each stale row outranks every pool member, so its absence is not a cut.
    let base_layer = &view.layers()[0];
    let q = scalar::encode(&view.layout(), &base_layer.scales, &query)
        .unwrap()
        .codes;
    let best_member = ranking
        .pool
        .ranking
        .ranked
        .iter()
        .map(|row| row.score)
        .fold(f64::NEG_INFINITY, f64::max);
    for stale in ["gamma", "beta"] {
        let (_, (_, row)) = winner(&view, 0, stale);
        let score = weighted(&base_layer.scales, &q, &base_layer.codes(row).unwrap());
        assert!(
            score > best_member,
            "{stale} would lead the pool if admitted"
        );
        assert!(
            !ranking
                .pool
                .winners
                .iter()
                .any(|winner| (winner.layer, winner.row) == (0, row)),
            "{stale}'s base row is no pool entry"
        );
        assert!(!reads.contains(&(base_layer.digest.clone(), row)));
    }
    let mut vectors: BTreeMap<String, Vec<f32>> = map(&corpus).into_iter().collect();
    vectors.insert(occurrence_id("gamma"), poor);
    vectors.remove(&occurrence_id("beta"));
    assert_eq!(
        rescored(&ranking),
        top_of_pool(&query, &ranking, &vectors, 4)
    );
}

/// Three occurrences carry one payload and lead the query. A pool of two cuts inside the tie and keeps the two smallest identifiers; a ranking of one keeps the smallest; equal payloads stay distinct entries.
#[test]
fn a_tie_across_the_pool_and_ranking_cuts_keeps_identifier_order() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let mut corpus = corpus();
    let lead = unit([0.98, 0.02, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let tied = ["alpha", "beta", "gamma"];
    for (object, vector) in &mut corpus {
        if tied.contains(object) {
            *vector = lead.clone();
        }
    }
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let mut ids: Vec<String> = tied.iter().map(|object| occurrence_id(object)).collect();
    ids.sort();

    let (ranking, reads) = rank_simple(&fixture, &projection, &view, &axis(0), 1, 2);
    let pool: Vec<String> = pool_keys(&ranking).into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        pool,
        ids[..2],
        "the pool cut keeps the two smallest identifiers"
    );
    assert_eq!(reads.len(), 2);
    let scores: BTreeSet<u64> = pool_keys(&ranking)
        .into_iter()
        .map(|(_, bits)| bits)
        .collect();
    assert_eq!(scores.len(), 1, "the kept entries tie exactly");
    assert_eq!(
        rescored(&ranking)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        ids[..1],
        "the ranking cut keeps the smallest identifier"
    );

    let (wide, _) = rank_simple(&fixture, &projection, &view, &axis(0), 3, 3);
    assert_eq!(
        rescored(&wide)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        ids,
        "each equal payload keeps its own occurrence"
    );
}

/// With no earlier verified composition, a member that fails verification leaves recovery and acquisition unavailable, and its bytes never serve. A rebuilt layer published under a new composition then serves, and every read names the rebuilt member.
#[test]
fn a_corrupt_composition_with_no_prior_state_serves_nothing_until_rebuilt() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let corpus = corpus();
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    fixture.corrupt(&base.digest, ROWS_FILE);

    assert!(matches!(
        fixture.recover(),
        Err(daemon::vector_composition::Unavailable::NoCompatibleTarget { .. })
    ));
    assert!(matches!(
        acquire_view(&mut fixture, &mut |_| {}),
        Err(daemon::vector_reader::AcquireRefusal::Unavailable(
            daemon::vector_composition::Unavailable::NoCompatibleTarget { .. }
        ))
    ));

    let mut rebuilt = corpus.clone();
    rebuilt[0].1 = axis(6);
    let fresh = fixture.layer_from(&export(&rebuilt, &[], 20));
    assert_ne!(fresh.digest, base.digest);
    fixture
        .publish(&fixture.compose(2, &fresh, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    assert_eq!(view.members(), vec![fresh.digest.clone()]);
    let query = axis(0);
    let (ranking, reads) = rank_simple(&fixture, &projection, &view, &query, 2, 3);
    assert!(reads.iter().all(|(member, _)| *member == fresh.digest));
    let mut expected = reference(&query, &map(&rebuilt));
    expected.truncate(2);
    assert_eq!(rescored(&ranking), expected);
}

const RESTART_CHILD: &str = "EIDNARA_RESCORE_RESTART_CHILD";
const RESTART_WITNESS: &str = "EIDNARA_RESCORE_RESTART_WITNESS";

/// A child process publishes one composition, publishes its replacement, cuts the replacement's rows to the header, and exits without running destructors, leaving its store. This process reopens the store with a fresh gate and ledger: recovery refuses the cut member and takes the earlier verified composition, its rows serve under the projection's current eligibility, and the cut member is never read.
#[test]
fn recovery_in_a_new_process_refuses_the_cut_member_and_serves_the_prior_set() {
    if std::env::var_os(RESTART_CHILD).is_some() {
        let mut fixture = Fixture::new();
        let corpus = corpus();
        let old_base = fixture.layer_from(&export(&corpus, &[], 10));
        let old = fixture
            .publish(&fixture.compose(1, &old_base, &[]).unwrap())
            .unwrap();
        let (_, new_base) = publish_replacement(&fixture, &corpus, 2);
        let rows = fixture.generation_dir(&new_base.digest).join(ROWS_FILE);
        let bytes = std::fs::read(&rows).unwrap();
        std::fs::write(&rows, &bytes[..ARTIFACT_HEADER_BYTES]).unwrap();
        fixture.root.keep();
        let witness = std::env::var_os(RESTART_WITNESS).unwrap();
        std::fs::write(
            witness,
            format!(
                "{}\n{}\n{}\n{}",
                fixture.root.path().display(),
                old,
                old_base.digest,
                new_base.digest
            ),
        )
        .unwrap();
        // The process ends without running destructors, as a daemon that dies holds its locks and handles until the kernel reclaims them.
        std::process::exit(0);
    }
    let scratch = tempfile::tempdir().unwrap();
    let witness = scratch.path().join("witness");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "recovery_in_a_new_process_refuses_the_cut_member_and_serves_the_prior_set",
            "--nocapture",
        ])
        .env(RESTART_CHILD, "1")
        .env(RESTART_WITNESS, &witness)
        .env("TMPDIR", scratch.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let witnessed = std::fs::read_to_string(&witness).unwrap();
    let [root, old, old_base, new_base] = witnessed.lines().collect::<Vec<_>>()[..] else {
        panic!("malformed witness {witnessed:?}");
    };

    let mut fixture = Fixture::reopen(std::path::Path::new(root));
    let projection = projection(&fixture, &OBJECTS);
    assert!(
        matches!(
            fixture.store.read_vector_current().unwrap(),
            host_runtime::generation::CurrentProfile::Current(ref selected) if selected != old
        ),
        "the child left its replacement selected"
    );
    let (recovered, selector) = fixture.recover().unwrap();
    assert_eq!(recovered, old);
    assert!(matches!(selector, SelectorState::Stale(_)));
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    assert_eq!(view.digest(), old);
    assert_eq!(view.members(), vec![old_base.to_owned()]);

    projection.retire("alpha");
    let query = axis(0);
    let (ranking, reads) = rank_simple(&fixture, &projection, &view, &query, 2, 3);
    assert!(!reads.is_empty());
    assert_ne!(old_base, new_base);
    assert!(reads.iter().all(|(member, _)| member == old_base));
    let mut eligible = map(&corpus());
    eligible.retain(|(id, _)| *id != occurrence_id("alpha"));
    let mut expected = reference(&query, &eligible);
    expected.truncate(2);
    assert_eq!(rescored(&ranking), expected);
}

/// Equal payloads in two layers: the smallest tied identifier sits in a delta and the larger ones in the base, so layer and row order disagree with identifier order. The pool admits every tied row, and the ranking cut keeps the smallest identifiers, whatever layer holds them.
#[test]
fn a_tie_across_layers_is_cut_by_identifier_not_by_layer_or_row() {
    let mut fixture = Fixture::new();
    let projection = projection(&fixture, &OBJECTS);
    let lead = unit([0.98, 0.02, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let tied = ["alpha", "beta", "gamma"];
    let mut ids: Vec<(String, &str)> = tied
        .iter()
        .map(|object| (occurrence_id(object), *object))
        .collect();
    ids.sort();
    let smallest = ids[0].1;
    let mut corpus = corpus();
    for (object, vector) in &mut corpus {
        if tied.contains(object) && *object != smallest {
            *vector = lead.clone();
        }
    }
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    let delta = fixture.layer_from(&export(&[(smallest, lead.clone())], &[], 12));
    let delta_digest = delta.digest.clone();
    fixture
        .publish(&fixture.compose(1, &base, &[delta]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let (smallest_id, delta_row) = winner(&view, 1, smallest);
    let ordered: Vec<String> = ids.iter().map(|(id, _)| id.clone()).collect();
    for k in 1..=3 {
        let (ranking, reads) = rank_simple(&fixture, &projection, &view, &axis(0), k, 5);
        let at = ranking
            .pool
            .ranking
            .ranked
            .iter()
            .position(|row| row.occurrence_id == smallest_id)
            .expect("the smallest tied identifier is pooled");
        assert_eq!(
            (ranking.pool.winners[at].layer, ranking.pool.winners[at].row),
            delta_row,
            "k {k}: the smallest tied identifier wins from the delta"
        );
        assert!(
            reads.contains(&(delta_digest.clone(), delta_row.1)),
            "k {k}: its original row is read from the delta: {reads:?}"
        );
        let pool: BTreeSet<String> = pool_keys(&ranking).into_iter().map(|(id, _)| id).collect();
        assert!(
            ordered.iter().all(|id| pool.contains(id)),
            "the pool admits every tied row"
        );
        assert_eq!(
            rescored(&ranking)
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            ordered[..k],
            "k {k}: the cut follows identifier order"
        );
    }
}

/// One composition carries every hazard at once: the query's best row (`alpha`) is hidden by the kernel, a delta calibrated on its own rows supersedes `gamma` and tombstones `beta`, and the delta's `gamma` carries an original vector equal to the base's `epsilon`. The pool equals one global prediction over the eligible winners under each winner's own layer scales; the hidden leader is neither admitted nor read; and the rescore over the pool keeps the tie in identifier order.
#[test]
fn a_rejected_leader_across_unequal_scale_layers_with_ties_fills_one_global_pool() {
    let mut fixture = Fixture::new();
    let admitted: Vec<&str> = OBJECTS.iter().copied().filter(|o| *o != "alpha").collect();
    let projection = projection(&fixture, &admitted);
    let mut corpus = corpus();
    corpus[0].1 = unit([0.99, 0.01, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let shared = unit([0.6, 0.0, 0.0, 0.0, 0.0, 0.8, 0.0, 0.0]);
    corpus[4].1 = shared.clone();
    let base = fixture.layer_from(&export(&corpus, &[], 10));
    // The delta's rows span other coordinates, so its calibration differs from the base's.
    let delta_rows: Vec<(&str, Vec<f32>)> = vec![
        ("gamma", shared.clone()),
        ("delta", unit([0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.98, 0.0])),
    ];
    let delta = fixture.layer_from(&export(&delta_rows, &["beta"], 12));
    fixture
        .publish(&fixture.compose(1, &base, &[delta]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    assert_ne!(
        view.layers()[0].scales.as_slice(),
        view.layers()[1].scales.as_slice(),
        "the layers carry unequal scales"
    );
    let query = axis(0);

    let (ranking, reads) = rank_simple(&fixture, &projection, &view, &query, 3, 3);
    let eligible: BTreeMap<String, (usize, usize)> = [
        winner(&view, 1, "gamma"),
        winner(&view, 1, "delta"),
        winner(&view, 0, "epsilon"),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        pool_keys(&ranking),
        expected_pool(&view, &query, &eligible, 3),
        "one global pool over the eligible winners, each under its own layer's scales"
    );
    let alpha = winner(&view, 0, "alpha").1;
    assert!(
        !ranking
            .pool
            .winners
            .iter()
            .any(|w| (w.layer, w.row) == alpha),
        "the hidden leader is not admitted"
    );
    assert!(!reads.contains(&(view.layers()[0].digest.clone(), alpha.1)));
    // Each original read names the member and row its pool entry won from, so the tied rows are read from their own layers.
    let expected_reads: Vec<(String, usize)> = ranking
        .pool
        .winners
        .iter()
        .map(|w| (view.layers()[w.layer].digest.clone(), w.row))
        .collect();
    assert_eq!(reads, expected_reads);
    // A pool of two cuts the eligible set across the unequal-scale layers; the cut follows the global prediction.
    let (cut, _) = rank_simple(&fixture, &projection, &view, &query, 2, 2);
    assert_eq!(pool_keys(&cut), expected_pool(&view, &query, &eligible, 2));
    let mut vectors: BTreeMap<String, Vec<f32>> = map(&corpus).into_iter().collect();
    for (object, row) in &delta_rows {
        vectors.insert(occurrence_id(object), row.clone());
    }
    let ranked = rescored(&ranking);
    assert_eq!(ranked, top_of_pool(&query, &ranking, &vectors, 3));
    // `gamma` and `epsilon` share an original row, so they tie exactly and lead `delta`; the smaller identifier comes first.
    let (g, e) = (occurrence_id("gamma"), occurrence_id("epsilon"));
    let (first, second) = if g < e { (g, e) } else { (e, g) };
    assert_eq!(
        ranked.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
        vec![first, second, occurrence_id("delta")]
    );
    assert_eq!(ranked[0].1.to_bits(), ranked[1].1.to_bits());
}

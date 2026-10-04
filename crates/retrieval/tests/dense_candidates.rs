//! RP2.6.U2 witnesses: the candidate scan over resolved layers against the real kernel.
//! Every expected pool is an independent full sort of the eligible resolved set `E` under the weighted int8 formula restated here; the codes come from the production quantizer, which RP2.5 owns, and nothing else does.

mod support;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use kernel::EligibilityVerdict;
use kernel::applicability::EvalBudget;
use retrieval::dense::candidates::select_candidates_with_hook_for_test;
use retrieval::dense::oracle::{HELD_SLOT_BYTES, held_bytes, selected_bytes};
use retrieval::dense::scalar::{QueryRefusal, Scales, calibrate, encode};
use retrieval::dense::{
    CandidateCapacity, CandidatePolicy, CandidatePool, CandidateQuery, CandidateRefusal,
    Completion, DenseCoverage, IncompleteReason, Layer, LayerCodes, LayeredRefusal, Metric,
    OracleRefusal, RescoreRefusal, ScanBounds, StorageBounds, Window, WinnerRow, rescore_pool,
};
use retrieval::eligibility::OccurrenceCandidate;

use support::dense::*;
use support::layers::OwnedLayer;

/// One layer's f32 rows plus the codes the production quantizer gives them under the layer's own calibration.
struct Coded {
    layer: OwnedLayer,
    scales: Scales,
    codes: Vec<Vec<i8>>,
}

impl Coded {
    fn new(
        fixture: &Fixture,
        ordinal: u32,
        rows: &[(&str, Vec<f32>)],
        tombstones: &[&str],
    ) -> Self {
        let layer = OwnedLayer::new(
            generation().generation_epoch,
            ordinal,
            rows.iter()
                .map(|(object, vector)| (fixture.id(object), vector.clone()))
                .collect(),
            tombstones.iter().map(|object| fixture.id(object)).collect(),
        );
        Self::from_layer(layer)
    }

    fn from_layer(layer: OwnedLayer) -> Self {
        let scales = calibrate(&layout(), layer.rows.iter().map(Vec::as_slice))
            .unwrap()
            .scales;
        let codes = layer
            .rows
            .iter()
            .map(|row| encode(&layout(), &scales, row).unwrap().codes)
            .collect();
        Self {
            layer,
            scales,
            codes,
        }
    }

    fn codes(&self) -> LayerCodes<'_> {
        LayerCodes {
            scales: &self.scales,
            codes: &self.codes,
        }
    }
}

fn full_base(fixture: &Fixture) -> Coded {
    let rows: Vec<(&str, Vec<f32>)> = fixture
        .rows
        .iter()
        .filter_map(|row| Some((row.object.as_str(), row.vector.clone()?)))
        .collect();
    Coded::new(fixture, 0, &rows, &[])
}

fn capacity(candidates: usize) -> CandidateCapacity {
    CandidateCapacity::new(
        1,
        CandidatePolicy {
            alpha: candidates as f64,
            cap: NonZeroUsize::new(candidates).unwrap(),
        },
    )
    .unwrap()
    .unwrap()
}

fn roomy(page_rows: usize) -> ScanBounds {
    ScanBounds {
        page_rows: NonZeroUsize::new(page_rows).unwrap(),
        max_rows: NonZeroUsize::new(4096).unwrap(),
        storage: StorageBounds {
            batch_bytes: NonZeroUsize::new(1 << 20).unwrap(),
            heap_bytes: NonZeroUsize::new(1 << 20).unwrap(),
        },
    }
}

fn select<'a>(
    fixture: &'a Fixture,
    layers: &'a [Layer<'a>],
    codes: &'a [LayerCodes<'a>],
    query: &'a [f32],
    candidates: usize,
    bounds: ScanBounds,
    hook: impl FnMut(Window<'_>),
) -> Result<CandidatePool, CandidateRefusal> {
    let generation = generation();
    let request = CandidateQuery {
        generation: &generation,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: TOLERANCE,
        query,
        authority: fixture.authority(),
        capacity: capacity(candidates),
        bounds,
        layers,
        codes,
        max_entries: NonZeroUsize::new(4096).unwrap(),
    };
    fixture
        .store
        .with_conn(|conn| {
            Ok(select_candidates_with_hook_for_test(
                conn,
                &fixture.kernel,
                &request,
                &EvalBudget::unbounded(),
                hook,
            ))
        })
        .unwrap()
}

/// `sum_j (s_j * s_j) * (q_j * d_j)` in f64, increasing coordinate order.
fn reference_score(scales: &Scales, query: &[i8], doc: &[i8]) -> f64 {
    let mut total = 0.0f64;
    for index in 0..query.len() {
        let scale = f64::from(scales.as_slice()[index]);
        total += scale * scale * f64::from(i32::from(query[index]) * i32::from(doc[index]));
    }
    total
}

/// `Top(R, E, quantized)`: every eligible winner, given by `object -> (layer index, row index)`, scored under its own layer and fully sorted.
fn reference_pool(
    fixture: &Fixture,
    coded: &[Coded],
    query: &[f32],
    eligible: &BTreeMap<&str, (usize, usize)>,
    candidates: usize,
) -> Vec<(String, u64)> {
    let mut scored: Vec<(String, f64)> = eligible
        .iter()
        .map(|(object, (layer, row))| {
            let layer = &coded[*layer];
            let q = encode(&layout(), &layer.scales, query).unwrap().codes;
            (
                fixture.id(object),
                reference_score(&layer.scales, &q, &layer.codes[*row]),
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

fn pool_of(pool: &CandidatePool) -> Vec<(String, u64)> {
    pool.ranking
        .ranked
        .iter()
        .map(|row| (row.occurrence_id.clone(), row.score.to_bits()))
        .collect()
}

/// The identities of `ranked`, `candidates`, and the layer rows `winners` name agree position by position.
fn assert_aligned(coded: &[Coded], pool: &CandidatePool) {
    assert_eq!(pool.ranking.ranked.len(), pool.ranking.candidates.len());
    assert_eq!(pool.ranking.ranked.len(), pool.winners.len());
    for ((row, candidate), winner) in pool
        .ranking
        .ranked
        .iter()
        .zip(&pool.ranking.candidates)
        .zip(&pool.winners)
    {
        assert_eq!(row.occurrence_id, candidate.occurrence_id);
        assert_eq!(row.occurrence_id, coded[winner.layer].layer.ids[winner.row]);
    }
}

/// A discarded pool keeps no row, no candidate, no winner, and no authority stamp.
fn assert_discarded(pool: &CandidatePool) {
    assert!(pool.ranking.ranked.is_empty());
    assert!(pool.ranking.candidates.is_empty());
    assert!(pool.winners.is_empty());
    assert!(pool.ranking.snapshot.is_none() && pool.ranking.incarnation.is_none());
}

/// Corpus objects with a vector, each at its row in the base layer.
fn base_positions(fixture: &Fixture, base: &Coded) -> BTreeMap<&'static str, (usize, usize)> {
    OBJECTS
        .into_iter()
        .filter_map(|object| {
            let id = fixture.id(object);
            let row = base
                .layer
                .ids
                .iter()
                .position(|candidate| *candidate == id)?;
            Some((object, (0, row)))
        })
        .collect()
}

#[test]
fn a_stable_scan_returns_exactly_the_top_r_of_the_eligible_resolved_set() {
    // `alpha` and `gamma` lead against the axis; leaving them unadmitted interleaves rejected and eligible rows in every batch.
    let admitted: Vec<&str> = OBJECTS
        .into_iter()
        .filter(|object| !matches!(*object, "alpha" | "gamma"))
        .collect();
    let fixture = Fixture::new(&admitted, corpus());
    let coded = [full_base(&fixture)];
    let layers = [coded[0].layer.layer()];
    let codes = [coded[0].codes()];
    let query = axis(0);
    let mut eligible = base_positions(&fixture, &coded[0]);
    eligible.retain(|object, _| admitted.contains(object));
    for candidates in [1, 3, 5, 6, 20] {
        for page_rows in [1, 2, 8] {
            let pool = select(
                &fixture,
                &layers,
                &codes,
                &query,
                candidates,
                roomy(page_rows),
                |_| {},
            )
            .unwrap();
            let expected = reference_pool(&fixture, &coded, &query, &eligible, candidates);
            assert!(!expected.is_empty());
            assert_eq!(pool_of(&pool), expected, "R {candidates}, page {page_rows}");
            assert_eq!(pool.ranking.completion, Completion::Complete);
            assert_aligned(&coded, &pool);
            for hidden in ["alpha", "gamma"] {
                assert!(
                    !pool_of(&pool)
                        .iter()
                        .any(|(id, _)| *id == fixture.id(hidden))
                );
            }
        }
    }
    let underfilled = select(&fixture, &layers, &codes, &query, 20, roomy(2), |_| {}).unwrap();
    assert_eq!(
        underfilled.ranking.ranked.len(),
        eligible.len(),
        "a pool larger than E holds E and nothing fabricated"
    );
}

/// Forty rows; the twenty best by score are hidden. With `R = 4` and four-row pages, the rejected prefix is longer than both the pool and the batch.
fn hidden_prefix() -> (Fixture, Vec<String>, Vec<String>) {
    let names: Vec<String> = (0..40).map(|index| format!("claim-{index:02}")).collect();
    let rows: Vec<Row> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let first = 0.95 - 0.02 * index as f32;
            Row::claim(
                name,
                unit([first, 1.0 - first, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            )
        })
        .collect();
    let objects: Vec<&str> = names.iter().map(String::as_str).collect();
    let (hidden, admitted) = objects.split_at(20);
    let fixture = Fixture::with_objects(&objects, admitted, rows);
    (
        fixture,
        hidden.iter().map(|name| (*name).to_owned()).collect(),
        admitted.iter().map(|name| (*name).to_owned()).collect(),
    )
}

fn coded_rows(fixture: &Fixture) -> Coded {
    let layer = OwnedLayer::new(
        generation().generation_epoch,
        0,
        fixture
            .rows
            .iter()
            .map(|row| (row.occurrence_id(), row.vector.clone().unwrap()))
            .collect(),
        vec![],
    );
    Coded::from_layer(layer)
}

#[test]
fn a_rejected_prefix_longer_than_the_pool_and_the_batch_does_not_starve_the_eligible_suffix() {
    let (fixture, hidden, admitted) = hidden_prefix();
    let coded = [coded_rows(&fixture)];
    let layers = [coded[0].layer.layer()];
    let codes = [coded[0].codes()];
    let query = axis(0);
    let positions: BTreeMap<&str, (usize, usize)> = admitted
        .iter()
        .map(|name| {
            let id = fixture.id(name);
            let row = coded[0].layer.ids.iter().position(|c| *c == id).unwrap();
            (name.as_str(), (0, row))
        })
        .collect();
    let expected = reference_pool(&fixture, &coded, &query, &positions, 4);
    for page_rows in [4, 40] {
        let pool = select(
            &fixture,
            &layers,
            &codes,
            &query,
            4,
            roomy(page_rows),
            |_| {},
        )
        .unwrap();
        assert_eq!(pool.ranking.completion, Completion::Complete);
        assert_eq!(pool_of(&pool), expected, "page {page_rows}");
        assert_aligned(&coded, &pool);
        let excluded: usize = pool
            .ranking
            .consumed
            .excluded
            .iter()
            .filter(|(verdict, _)| *verdict == EligibilityVerdict::Hidden)
            .map(|(_, count)| count)
            .sum();
        assert!(excluded >= 4, "rejected leaders were judged: {excluded}");
        if page_rows == 40 {
            // One page: the first batch is the four best rows, all hidden; it admits nothing, so the rest of the page is one batch; then the re-judgment.
            assert_eq!(
                pool.ranking.consumed.excluded,
                vec![(EligibilityVerdict::Hidden, 20)]
            );
            assert_eq!(pool.ranking.consumed.batches, 3);
        }
    }

    // Negative control: the unchecked top-R over every row, filtered afterwards, finds nothing.
    let mut everyone: BTreeMap<&str, (usize, usize)> = positions.clone();
    for name in &hidden {
        let id = fixture.id(name);
        let row = coded[0].layer.ids.iter().position(|c| *c == id).unwrap();
        everyone.insert(name.as_str(), (0, row));
    }
    let unchecked = reference_pool(&fixture, &coded, &query, &everyone, 4);
    let hidden_ids: Vec<String> = hidden.iter().map(|name| fixture.id(name)).collect();
    let filtered: Vec<_> = unchecked
        .into_iter()
        .filter(|(id, _)| !hidden_ids.contains(id))
        .collect();
    assert!(filtered.is_empty(), "top-R-then-filter starves the suffix");
}

#[test]
fn only_resolved_winners_are_scored_and_each_scores_under_its_own_layer() {
    let fixture = Fixture::all_admitted();
    let base = full_base(&fixture);
    // The delta replaces `delta` with a vector the axis prefers and tombstones `alpha`; its own calibration gives it other scales.
    let replaced = unit([0.99, 0.01, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let delta = Coded::new(
        &fixture,
        1,
        &[
            ("delta", replaced.clone()),
            ("theta", fixture.row("theta").vector.clone().unwrap()),
        ],
        &["alpha"],
    );
    assert_ne!(base.scales, delta.scales);
    let coded = [base, delta];
    let layers: Vec<Layer<'_>> = coded.iter().map(|c| c.layer.layer()).collect();
    let codes: Vec<LayerCodes<'_>> = coded.iter().map(Coded::codes).collect();
    let query = axis(0);
    let mut eligible = base_positions(&fixture, &coded[0]);
    eligible.remove("alpha");
    let delta_row = |object: &str| {
        let id = fixture.id(object);
        coded[1].layer.ids.iter().position(|c| *c == id).unwrap()
    };
    eligible.insert("delta", (1, delta_row("delta")));
    eligible.insert("theta", (1, delta_row("theta")));
    let pool = select(&fixture, &layers, &codes, &query, 8, roomy(2), |_| {}).unwrap();
    assert_eq!(
        pool_of(&pool),
        reference_pool(&fixture, &coded, &query, &eligible, 8)
    );
    assert_eq!(pool.ranking.ranked[0].occurrence_id, fixture.id("delta"));
    assert_eq!(pool.winners[0].layer, 1, "the delta's row won");
    assert!(
        !pool_of(&pool)
            .iter()
            .any(|(id, _)| *id == fixture.id("alpha"))
    );
    assert_eq!(pool.layers.masked, 1);
    assert_eq!(pool.layers.superseded, 2);
    assert_eq!(
        pool.ranking.coverage.with_vector,
        eligible.len(),
        "one score per winner, none for a superseded or masked row"
    );
    assert_aligned(&coded, &pool);
}

#[test]
fn a_kernel_change_between_batches_discards_the_pool() {
    let fixture = Fixture::all_admitted();
    let coded = [full_base(&fixture)];
    let layers = [coded[0].layer.layer()];
    let codes = [coded[0].codes()];
    let query = axis(0);
    let pool = select(&fixture, &layers, &codes, &query, 8, roomy(2), |window| {
        if window == Window::AfterPage(1) {
            fixture.retire("theta");
        }
    })
    .unwrap();
    assert_eq!(
        pool.ranking.completion,
        Completion::Incomplete(IncompleteReason::SnapshotChanged)
    );
    assert_discarded(&pool);

    let manifest = fixture.backup();
    let pool = select(&fixture, &layers, &codes, &query, 8, roomy(2), |window| {
        if window == Window::AfterPage(1) {
            fixture.kernel.restore(&manifest).unwrap();
        }
    })
    .unwrap();
    assert_eq!(
        pool.ranking.completion,
        Completion::Incomplete(IncompleteReason::KernelIncarnationChanged)
    );
    assert_discarded(&pool);
}

/// Admitting `alpha` after the walk judged it hidden moves the snapshot: the exclusion described facts that no longer hold, so the pool cannot be complete.
#[test]
fn a_change_to_an_excluded_row_discards_the_pool_too() {
    let admitted: Vec<&str> = OBJECTS.into_iter().filter(|o| *o != "alpha").collect();
    let fixture = Fixture::new(&admitted, corpus());
    let coded = [full_base(&fixture)];
    let layers = [coded[0].layer.layer()];
    let codes = [coded[0].codes()];
    let query = axis(0);
    let mut judged = 0;
    let pool = select(&fixture, &layers, &codes, &query, 8, roomy(8), |window| {
        if window == Window::AfterJudgment {
            judged += 1;
            if judged == 1 {
                fixture
                    .kernel
                    .commit(intent("admit-alpha"), |envelope| {
                        envelope.record_admission(admission("alpha"))?;
                        Ok(String::new())
                    })
                    .unwrap();
            }
        }
    })
    .unwrap();
    assert_eq!(
        pool.ranking.consumed.excluded,
        vec![(EligibilityVerdict::Hidden, 1)],
        "alpha was judged hidden before it was admitted"
    );
    assert_eq!(
        pool.ranking.completion,
        Completion::Incomplete(IncompleteReason::SnapshotChanged)
    );
    assert_discarded(&pool);
}

#[test]
fn a_corrupt_identity_field_refuses_the_scan_with_no_candidate() {
    let fixture = Fixture::all_admitted();
    let coded = [full_base(&fixture)];
    let layers = [coded[0].layer.layer()];
    let codes = [coded[0].codes()];
    fixture
        .raw()
        .execute(
            "UPDATE occurrences SET source_object_id='' WHERE occurrence_id=?1",
            rusqlite::params![fixture.id("theta")],
        )
        .unwrap();
    assert_eq!(
        select(&fixture, &layers, &codes, &axis(0), 8, roomy(2), |_| {}),
        Err(CandidateRefusal::Layered(LayeredRefusal::Oracle(
            OracleRefusal::Kernel(kernel::KernelError::InvalidInput)
        )))
    );
}

#[test]
fn each_storage_and_row_bound_saturates_alone_and_returns_no_candidate() {
    let fixture = Fixture::all_admitted();
    let coded = [full_base(&fixture)];
    let layers = [coded[0].layer.layer()];
    let codes = [coded[0].codes()];
    let query = axis(0);
    let run = |candidates: usize, bounds: ScanBounds| {
        select(
            &fixture,
            &layers,
            &codes,
            &query,
            candidates,
            bounds,
            |_| {},
        )
    };
    let complete = run(3, roomy(8)).unwrap();
    assert_eq!(complete.ranking.completion, Completion::Complete);
    let held: Vec<OccurrenceCandidate> = complete.ranking.candidates.clone();

    // Scan work: one row short of the population.
    let population = complete.ranking.coverage.required;
    let short = run(
        3,
        ScanBounds {
            max_rows: NonZeroUsize::new(population - 1).unwrap(),
            ..roomy(2)
        },
    )
    .unwrap();
    assert_eq!(
        short.ranking.completion,
        Completion::Incomplete(IncompleteReason::RowBound)
    );
    assert_discarded(&short);
    let exact = run(
        3,
        ScanBounds {
            max_rows: NonZeroUsize::new(population).unwrap(),
            ..roomy(2)
        },
    )
    .unwrap();
    assert_eq!(exact.ranking.completion, Completion::Complete);

    // Batch bytes. With a pool of eight every row is selected, so a one-row page holds the largest row and no more.
    let every = run(8, roomy(8)).unwrap().ranking.candidates;
    assert_eq!(every.len(), 8);
    let largest = every.iter().map(selected_bytes).max().unwrap();
    let batch = |page_rows: usize, batch_bytes: usize| {
        run(
            8,
            ScanBounds {
                storage: StorageBounds {
                    batch_bytes: NonZeroUsize::new(batch_bytes).unwrap(),
                    ..roomy(page_rows).storage
                },
                ..roomy(page_rows)
            },
        )
        .unwrap()
    };
    assert_eq!(batch(1, largest).ranking.completion, Completion::Complete);
    let short = batch(1, largest - 1);
    assert_eq!(
        short.ranking.completion,
        Completion::Incomplete(IncompleteReason::BatchBytes)
    );
    assert_discarded(&short);
    // Each page starts its count afresh: two rows per page fit twice the largest row, below the scan's total.
    assert!(every.iter().map(selected_bytes).sum::<usize>() > 2 * largest);
    assert_eq!(
        batch(2, 2 * largest).ranking.completion,
        Completion::Complete
    );

    // Heap bytes, with one-row pages so later rows displace held ones: the preallocated slots and one byte fit; the first admission's strings do not.
    let slots = 3 * HELD_SLOT_BYTES;
    let heap = run(
        3,
        ScanBounds {
            storage: StorageBounds {
                heap_bytes: NonZeroUsize::new(slots + 1).unwrap(),
                ..roomy(1).storage
            },
            ..roomy(1)
        },
    )
    .unwrap();
    assert_eq!(
        heap.ranking.completion,
        Completion::Incomplete(IncompleteReason::HeapBytes)
    );
    assert_discarded(&heap);
    // The peak the walk holds: rows visited in identifier order, one per page, each offered while it outranks the worst of three.
    let eligible = base_positions(&fixture, &coded[0]);
    let scored = reference_pool(&fixture, &coded, &query, &eligible, usize::MAX);
    let mut visit: Vec<(String, u64)> = scored.clone();
    visit.sort();
    let candidate_of = |id: &str| {
        let row = fixture
            .rows
            .iter()
            .find(|row| row.occurrence_id() == id)
            .unwrap();
        OccurrenceCandidate::new(
            id.to_owned(),
            row.class,
            row.object.clone(),
            1,
            DIGEST.to_owned(),
        )
    };
    let rank = |id: &str| scored.iter().position(|(other, _)| other == id).unwrap();
    let mut held_ids: Vec<String> = Vec::new();
    let mut peak = 0;
    for (id, _) in &visit {
        held_ids.push(id.clone());
        held_ids.sort_by_key(|id| rank(id));
        held_ids.truncate(3);
        peak = peak.max(
            held_ids
                .iter()
                .map(|id| held_bytes(&candidate_of(id)))
                .sum::<usize>(),
        );
    }
    let final_held: usize = held.iter().map(held_bytes).sum();
    assert!(
        peak > final_held,
        "a displaced entry owned longer strings than its replacement"
    );
    let heap_at = |bytes: usize| {
        run(
            3,
            ScanBounds {
                storage: StorageBounds {
                    heap_bytes: NonZeroUsize::new(bytes).unwrap(),
                    ..roomy(1).storage
                },
                ..roomy(1)
            },
        )
        .unwrap()
    };
    let fits = heap_at(slots + peak);
    assert_eq!(
        fits.ranking.completion,
        Completion::Complete,
        "the peak bound holds the walk"
    );
    assert_eq!(pool_of(&fits), pool_of(&complete));
    let paged = run(3, roomy(1)).unwrap();
    assert!(
        paged.ranking.consumed.judged - 3 > 3,
        "{:?}",
        paged.ranking.consumed
    );
    let tight = heap_at(slots + peak - 1);
    assert_eq!(
        tight.ranking.completion,
        Completion::Incomplete(IncompleteReason::HeapBytes)
    );

    // Slots that do not fit are refused before any page is read.
    let mut visited = 0;
    let refused = select(
        &fixture,
        &layers,
        &codes,
        &query,
        3,
        ScanBounds {
            storage: StorageBounds {
                heap_bytes: NonZeroUsize::new(slots - 1).unwrap(),
                ..roomy(8).storage
            },
            ..roomy(8)
        },
        |window| {
            if matches!(window, Window::Visited(_)) {
                visited += 1;
            }
        },
    );
    assert_eq!(
        refused,
        Err(CandidateRefusal::Layered(LayeredRefusal::Oracle(
            OracleRefusal::HeapOverBound {
                entries: 3,
                limit: slots - 1
            }
        )))
    );
    assert_eq!(visited, 0);

    // A pool past the kernel's batch is refused at the walk's entry.
    assert!(matches!(
        run(2000, roomy(8)),
        Err(CandidateRefusal::Layered(LayeredRefusal::Oracle(
            OracleRefusal::BatchOverBound {
                bound: "k",
                value: 2000
            }
        )))
    ));
}

#[test]
fn an_ended_budget_returns_no_candidate() {
    let fixture = Fixture::all_admitted();
    let coded = [full_base(&fixture)];
    let layers = [coded[0].layer.layer()];
    let codes = [coded[0].codes()];
    let generation = generation();
    let query = axis(0);
    let budget = EvalBudget::unbounded();
    let request = CandidateQuery {
        generation: &generation,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: TOLERANCE,
        query: &query,
        authority: fixture.authority(),
        capacity: capacity(3),
        bounds: roomy(2),
        layers: &layers,
        codes: &codes,
        max_entries: NonZeroUsize::new(64).unwrap(),
    };
    let pool = fixture
        .store
        .with_conn(|conn| {
            Ok(select_candidates_with_hook_for_test(
                conn,
                &fixture.kernel,
                &request,
                &budget,
                |window| {
                    if window == Window::AfterPage(1) {
                        budget.cancel();
                    }
                },
            ))
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        pool.ranking.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted)
    );
    assert_discarded(&pool);
}

#[test]
fn a_coverage_shortfall_keeps_the_pool_and_says_so() {
    let fixture = Fixture::all_admitted();
    let mut base = full_base(&fixture);
    let beta = fixture.id("beta");
    let row = base.layer.ids.iter().position(|id| *id == beta).unwrap();
    base.layer.ids.remove(row);
    base.layer.rows.remove(row);
    base.codes.remove(row);
    let coded = [base];
    let layers = [coded[0].layer.layer()];
    let codes = [coded[0].codes()];
    let pool = select(&fixture, &layers, &codes, &axis(0), 3, roomy(2), |_| {}).unwrap();
    assert_eq!(
        pool.ranking.completion,
        Completion::Incomplete(IncompleteReason::DenseCoverageShortfall)
    );
    let mut eligible = base_positions(&fixture, &coded[0]);
    eligible.remove("beta");
    assert_eq!(
        pool_of(&pool),
        reference_pool(&fixture, &coded, &axis(0), &eligible, 3)
    );
    assert_aligned(&coded, &pool);
    // A bound reached in the same walk names itself, and the pool goes.
    let bounded = select(
        &fixture,
        &layers,
        &codes,
        &axis(0),
        3,
        ScanBounds {
            max_rows: NonZeroUsize::new(pool.ranking.coverage.required - 1).unwrap(),
            ..roomy(2)
        },
        |_| {},
    )
    .unwrap();
    assert_eq!(
        bounded.ranking.completion,
        Completion::Incomplete(IncompleteReason::RowBound)
    );
    assert_discarded(&bounded);
}

#[test]
fn a_shortfall_counts_open_work_for_each_row_no_layer_holds() {
    let fixture = Fixture::all_admitted();
    let mut base = full_base(&fixture);
    for object in ["beta", "gamma"] {
        let id = fixture.id(object);
        let row = base.layer.ids.iter().position(|held| *held == id).unwrap();
        base.layer.ids.remove(row);
        base.layer.rows.remove(row);
        base.codes.remove(row);
    }
    // `alpha` is held, so its open job leaves its coverage alone; `beta` and `gamma` are missing, with and without open work.
    fixture.job_state("alpha", "pending");
    fixture.job_state("beta", "pending");
    let coded = [base];
    let layers = [coded[0].layer.layer()];
    let codes = [coded[0].codes()];
    let pool = select(&fixture, &layers, &codes, &axis(0), 3, roomy(2), |_| {}).unwrap();
    assert_eq!(
        pool.ranking.coverage,
        DenseCoverage {
            required: 8,
            with_vector: 6,
            missing_pending: 1,
            missing_without_pending: 1,
        }
    );
    assert_eq!(
        pool.ranking.completion,
        Completion::Incomplete(IncompleteReason::DenseCoverageShortfall)
    );
}

#[test]
fn an_ended_budget_outranks_a_batch_bound_reached_in_the_same_flush() {
    let fixture = Fixture::all_admitted();
    let coded = [full_base(&fixture)];
    let layers = [coded[0].layer.layer()];
    let codes = [coded[0].codes()];
    let generation = generation();
    let query = axis(0);
    let budget = EvalBudget::unbounded();
    let request = CandidateQuery {
        generation: &generation,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: TOLERANCE,
        query: &query,
        authority: fixture.authority(),
        capacity: capacity(8),
        bounds: ScanBounds {
            storage: StorageBounds {
                batch_bytes: NonZeroUsize::new(1).unwrap(),
                ..roomy(8).storage
            },
            ..roomy(8)
        },
        layers: &layers,
        codes: &codes,
        max_entries: NonZeroUsize::new(64).unwrap(),
    };
    let mut visited = 0;
    // The second visit cancels; the walk then flushes the first row, which the one-byte batch bound refuses.
    let pool = fixture
        .store
        .with_conn(|conn| {
            Ok(select_candidates_with_hook_for_test(
                conn,
                &fixture.kernel,
                &request,
                &budget,
                |window| {
                    if matches!(window, Window::Visited(_)) {
                        visited += 1;
                        if visited == 2 {
                            budget.cancel();
                        }
                    }
                },
            ))
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        pool.ranking.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted)
    );
    assert_discarded(&pool);
}

#[test]
fn codes_that_do_not_cover_their_layer_refuse_before_the_projection_is_read() {
    let fixture = Fixture::all_admitted();
    let mut base = full_base(&fixture);
    base.codes.pop();
    let coded = [base];
    let layers = [coded[0].layer.layer()];
    let codes = [coded[0].codes()];
    let rows = coded[0].layer.rows.len();
    let mut visited = 0;
    assert_eq!(
        select(&fixture, &layers, &codes, &axis(0), 3, roomy(2), |window| {
            if matches!(window, Window::Visited(_)) {
                visited += 1;
            }
        }),
        Err(CandidateRefusal::CodeRows {
            layer: 0,
            rows,
            codes: rows - 1
        })
    );
    assert_eq!(visited, 0);
}

#[test]
fn wiring_and_query_faults_refuse_before_the_projection_is_read() {
    let fixture = Fixture::all_admitted();
    let coded = [full_base(&fixture)];
    let layers = [coded[0].layer.layer()];
    let no_codes: [LayerCodes<'_>; 0] = [];
    let mut visited = 0;
    let count = |window: Window<'_>| {
        if matches!(window, Window::Visited(_)) {
            visited += 1;
        }
    };
    assert_eq!(
        select(&fixture, &layers, &no_codes, &axis(0), 3, roomy(2), count),
        Err(CandidateRefusal::Codes {
            layers: 1,
            codes: 0
        })
    );
    let ones = Scales::from_values(vec![1.0; 8], DIMENSION).unwrap();
    let zero_codes = [LayerCodes {
        scales: &ones,
        codes: &coded[0].codes,
    }];
    let spread = vec![0.353_553_4f32; 8];
    assert_eq!(
        select(&fixture, &layers, &zero_codes, &spread, 3, roomy(2), |_| {}),
        Err(CandidateRefusal::Query {
            layer: 0,
            refusal: QueryRefusal::ZeroCodes
        })
    );
    assert_eq!(visited, 0);
}

#[test]
fn a_reserved_stored_code_refuses_as_unreadable() {
    let fixture = Fixture::all_admitted();
    let mut base = full_base(&fixture);
    let alpha = fixture.id("alpha");
    let row = base.layer.ids.iter().position(|id| *id == alpha).unwrap();
    base.codes[row][3] = i8::MIN;
    let coded = [base];
    let layers = [coded[0].layer.layer()];
    let codes = [coded[0].codes()];
    let refused = select(&fixture, &layers, &codes, &axis(0), 3, roomy(2), |_| {});
    assert!(matches!(
        refused,
        Err(CandidateRefusal::Layered(LayeredRefusal::Oracle(
            OracleRefusal::Unreadable { ref occurrence_id, .. }
        ))) if *occurrence_id == alpha
    ));
}

/// A pool of `rows` in the given order, each its own winner row, with no stamps.
fn synthetic_pool(ids: &[&str]) -> CandidatePool {
    let candidates: Vec<OccurrenceCandidate> = ids
        .iter()
        .map(|id| {
            OccurrenceCandidate::new(
                (*id).to_owned(),
                kernel::source_identity::OccurrenceClass::CanonicalClaims,
                "object".to_owned(),
                1,
                DIGEST.to_owned(),
            )
        })
        .collect();
    CandidatePool {
        ranking: retrieval::dense::ExhaustiveRanking {
            ranked: candidates
                .iter()
                .map(|candidate| retrieval::dense::Ranked {
                    occurrence_id: candidate.occurrence_id.clone(),
                    class: candidate.class,
                    score: 0.0,
                })
                .collect(),
            candidates,
            completion: Completion::Complete,
            coverage: retrieval::dense::DenseCoverage::default(),
            snapshot: None,
            incarnation: None,
            consumed: retrieval::dense::Consumed::default(),
        },
        layers: retrieval::dense::LayerAccount::default(),
        winners: (0..ids.len())
            .map(|row| WinnerRow { layer: 0, row })
            .collect(),
    }
}

#[test]
fn the_rescore_reads_each_entry_once_in_pool_order_and_ranks_by_original_score_then_identifier() {
    let ids = ["d", "b", "c", "a"];
    let pool = synthetic_pool(&ids);
    let rows = [
        axis(1),
        axis(0),
        axis(0),
        unit([0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ];
    let mut reads = Vec::new();
    let rescored = rescore_pool(
        &pool,
        &layout(),
        &axis(0),
        NonZeroUsize::new(3).unwrap(),
        |winner, row| {
            reads.push(winner.row);
            row.clone_from(&rows[winner.row]);
            Ok::<_, ()>(())
        },
    )
    .unwrap();
    assert_eq!(reads, [0, 1, 2, 3]);
    let order: Vec<&str> = rescored
        .ranked
        .iter()
        .map(|row| row.occurrence_id.as_str())
        .collect();
    assert_eq!(
        order,
        ["b", "c", "a"],
        "equal scores keep identifier order; k cuts the rest"
    );
    assert_eq!(rescored.ranked[0].score, 1.0);
    for (row, candidate) in rescored.ranked.iter().zip(&rescored.candidates) {
        assert_eq!(row.occurrence_id, candidate.occurrence_id);
    }
}

#[test]
fn the_rescore_refuses_a_bad_query_before_any_read_and_stops_at_the_first_failed_or_malformed_row()
{
    let pool = synthetic_pool(&["a", "b", "c"]);
    let k = NonZeroUsize::new(3).unwrap();
    let mut reads = 0;
    let zero = vec![0.0f32; 8];
    assert_eq!(
        rescore_pool(&pool, &layout(), &zero, k, |_, row| {
            reads += 1;
            *row = axis(0);
            Ok::<_, ()>(())
        }),
        Err(RescoreRefusal::Query(
            retrieval::dense::RowRejection::ZeroNorm
        ))
    );
    assert_eq!(reads, 0);
    let mut reads = 0;
    let failed = rescore_pool(&pool, &layout(), &axis(0), k, |winner, row| {
        reads += 1;
        if winner.row == 1 {
            return Err("torn");
        }
        *row = axis(0);
        Ok(())
    });
    assert_eq!(
        failed,
        Err(RescoreRefusal::Read {
            occurrence_id: "b".to_owned(),
            winner: WinnerRow { layer: 0, row: 1 },
            fault: "torn"
        })
    );
    assert_eq!(reads, 2);
    let malformed = rescore_pool(&pool, &layout(), &axis(0), k, |winner, row| {
        *row = if winner.row == 2 {
            vec![2.0; 8]
        } else {
            axis(0)
        };
        Ok::<_, ()>(())
    });
    assert!(matches!(
        malformed,
        Err(RescoreRefusal::Row { ref occurrence_id, winner: WinnerRow { row: 2, .. }, .. }) if occurrence_id == "c"
    ));
}

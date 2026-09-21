//! The stage ledger over the activated query route and packer: seven injected
//! faults, one per chain stage, each classified from production returns.

#![cfg(feature = "test-support")]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::num::NonZeroUsize;

use daemon::packing::{ClaudeTokens, PreparationRefusal};
use daemon::query_route::{
    DenseLane, ExactAdmission, ExactReport, ExhaustiveProducer, LaneStatus, Phase, QueryFailure,
    QueryOutcome, QueryRouteLimits, admit_lanes, execute, select,
};
use eval_core::{
    ChainStage, Completed, Coverage, CoverageError, LedgerError, MARKERS,
    MAX_CANDIDATES_PER_STAGE_OBSERVATION, Presence, Required, StageVerdict,
};
use kernel::ArtifactDestination;
use kernel::applicability::EvalBudget;
use retrieval::eligibility::{Authority, Disposition};
use retrieval::fusion::Lane;
use retrieval::packing::{BoundExceeded, OptionalBound};
use support::eval_ledger::{
    ChainLedger, Incarnations, LaneView, observe_lanes, observe_outcome, observe_packing,
    observe_refusal, pack, packing_store, request, survivors,
};
use support::query_route::{
    Fixture, GENERATION, QUERY, dense_limits, dense_reference, entry_ids, limits, query_vector,
    request_budget, vector_for,
};

const SUITE: &str = "crates/daemon/tests/eval_ledger.rs::";
const EXACT_ONLY: &str = "id:rule";
const WIDE_TOKENS: ClaudeTokens = ClaudeTokens::new(1 << 20);

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

fn required(occurrence: &str, entry: ChainStage) -> Vec<Required<ChainStage>> {
    vec![Required {
        occurrence: occurrence.to_string(),
        entry,
    }]
}

/// One request through the shell: lanes observed from `Admitted`, then fusion
/// and selection from the outcome or the refusal.
struct Run {
    view: Option<LaneView>,
    outcome: Result<QueryOutcome, QueryFailure>,
    ledger: ChainLedger,
    incarnations: Incarnations,
}

fn drive(
    fixture: &Fixture,
    limits: &QueryRouteLimits,
    query: &str,
    dense: DenseLane<'_>,
    mut before_phase: impl FnMut(Phase),
) -> Run {
    let (_token, budget) = request_budget(10_000);
    let authority = Authority {
        project: &fixture.project,
        destination: ArtifactDestination::Local,
    };
    let mut ledger = ChainLedger::default();
    let mut incarnations = Incarnations::default();
    let admitted = match admit_lanes(
        &fixture.projection,
        &fixture.store,
        authority,
        limits,
        budget.shared(),
        query,
        dense,
        &mut before_phase,
    ) {
        Ok(admitted) => admitted,
        Err(failure) => {
            return Run {
                view: None,
                outcome: Err(failure),
                ledger,
                incarnations,
            };
        }
    };
    let view = LaneView::of(&admitted);
    observe_lanes(&mut ledger, &view, &mut incarnations);
    let outcome = select(admitted, before_phase);
    match &outcome {
        Ok(outcome) => observe_outcome(&mut ledger, outcome, &mut incarnations),
        Err(failure) => observe_refusal(&mut ledger, failure),
    }
    Run {
        view: Some(view),
        outcome,
        ledger,
        incarnations,
    }
}

fn plain(fixture: &Fixture, limits: &QueryRouteLimits, query: &str) -> Run {
    drive(fixture, limits, query, DenseLane::Undeclared, |_| {})
}

fn view(run: &Run) -> &LaneView {
    run.view.as_ref().expect("the lanes were admitted")
}

fn outcome(run: &Run) -> &QueryOutcome {
    run.outcome.as_ref().expect("the request completed")
}

fn exact_rows(run: &Run) -> Vec<String> {
    view(run)
        .exact
        .rows
        .iter()
        .map(|row| row.occurrence_id.clone())
        .collect()
}

fn fused_ids(run: &Run) -> Vec<String> {
    outcome(run)
        .fused
        .entries()
        .iter()
        .map(|entry| entry.occurrence().to_string())
        .collect()
}

/// The verdict for a query-only run: selection is the terminal stage.
fn verdict(run: &Run, occurrence: &str, entry: ChainStage) -> StageVerdict<ChainStage> {
    run.ledger.verdict(
        &required(occurrence, entry),
        &BTreeSet::new(),
        ChainStage::Selection,
    )
}

fn packed_verdict(ledger: &ChainLedger, occurrence: &str) -> StageVerdict<ChainStage> {
    ledger.verdict(
        &required(occurrence, ChainStage::Exact),
        &BTreeSet::new(),
        ChainStage::Packing,
    )
}

/// The response entries as optional packing requests, in response order, with
/// the revision the projection carries for each. Packing is the filter after
/// selection, so it receives what the capped response carries, not the fused
/// set the caps cut.
fn requests(fixture: &Fixture, run: &Run) -> Vec<retrieval::packing::RequiredRequest> {
    let revisions: BTreeMap<String, i64> = fixture
        .live_candidates()
        .into_iter()
        .map(|candidate| (candidate.occurrence_id, candidate.candidate.source_revision))
        .collect();
    entry_ids(&outcome(run).body)
        .iter()
        .map(|id| request(id, revisions[id]))
        .collect()
}

fn exact_page_bound_loses_the_rule_at_the_exact_lane(coverage: &mut Coverage) {
    block_on(async {
        let fixture = Fixture::build().await;
        let healthy = plain(&fixture, &limits(), EXACT_ONLY);
        let rows = exact_rows(&healthy);
        assert!(rows.len() >= 2, "{rows:?}");
        let rule = rows.last().unwrap().clone();
        assert_eq!(
            verdict(&healthy, &rule, ChainStage::Exact),
            StageVerdict::Clean
        );

        let mut bounded = limits();
        bounded.exact_pages = NonZeroUsize::MIN;
        bounded.exact_page_rows = NonZeroUsize::MIN;
        let run = plain(&fixture, &bounded, EXACT_ONLY);
        let read = exact_rows(&run);
        assert_eq!(
            *view(&run).status(Lane::Exact),
            LaneStatus::Incomplete("page_bound")
        );
        assert!(!read.is_empty() && !read.contains(&rule), "{read:?}");
        coverage.record("ldg_injection_exact_page_bound").unwrap();
        assert_eq!(
            verdict(&run, &rule, ChainStage::Exact),
            StageVerdict::FirstLoss(ChainStage::Exact)
        );
        fixture.daemon.shutdown().await;
    });
}

fn lexical_accepted_bound_loses_the_rule_at_the_lexical_lane(coverage: &mut Coverage) {
    block_on(async {
        let fixture = Fixture::build().await;
        let healthy = plain(&fixture, &limits(), QUERY);
        let exact = exact_rows(&healthy);
        let lexical = view(&healthy).ranking(Lane::Lexical).to_vec();
        assert!(lexical.len() >= 2, "{lexical:?}");
        let rule = lexical
            .iter()
            .rev()
            .find(|id| !exact.contains(id))
            .expect("a lexical-only hit exists")
            .clone();
        assert_eq!(
            verdict(&healthy, &rule, ChainStage::Lexical),
            StageVerdict::Clean
        );

        let mut bounded = limits();
        bounded.lexical_accepted = NonZeroUsize::MIN;
        let run = plain(&fixture, &bounded, QUERY);
        let ranked = view(&run).ranking(Lane::Lexical);
        assert_eq!(
            *view(&run).status(Lane::Lexical),
            LaneStatus::Incomplete("accepted_bound")
        );
        assert_eq!(ranked.len(), 1, "{ranked:?}");
        assert!(!ranked.contains(&rule));
        coverage
            .record("ldg_injection_lexical_accepted_bound")
            .unwrap();
        assert_eq!(
            verdict(&run, &rule, ChainStage::Lexical),
            StageVerdict::FirstLoss(ChainStage::Lexical)
        );
        fixture.daemon.shutdown().await;
    });
}

fn dense_k_bound_loses_the_rule_at_the_dense_lane(coverage: &mut Coverage) {
    block_on(async {
        let fixture = Fixture::build().await;
        let rows = fixture.store_vectors(vector_for);
        let query = query_vector();
        let reference = dense_reference(&query, &rows);
        let rule = reference[1].0.clone();
        let with_k = |k: usize| {
            let mut limits = limits();
            limits.dense = Some(dense_limits(k));
            let producer = ExhaustiveProducer {
                limits: dense_limits(k),
            };
            let lane = DenseLane::Ready {
                query: &query,
                generation_id: GENERATION,
                producer: &producer,
            };
            drive(&fixture, &limits, QUERY, lane, |_| {})
        };
        let healthy = with_k(8);
        let ranked = view(&healthy).ranking(Lane::Dense);
        assert_eq!(*view(&healthy).status(Lane::Dense), LaneStatus::Complete);
        assert_eq!(ranked.get(1), Some(&rule), "{ranked:?}");
        assert_eq!(
            verdict(&healthy, &rule, ChainStage::Dense),
            StageVerdict::Clean
        );

        let run = with_k(1);
        let ranked = view(&run).ranking(Lane::Dense);
        assert_eq!(*view(&run).status(Lane::Dense), LaneStatus::Complete);
        assert_eq!(ranked, [reference[0].0.clone()], "{ranked:?}");
        coverage.record("ldg_injection_dense_k_bound").unwrap();
        assert_eq!(
            verdict(&run, &rule, ChainStage::Dense),
            StageVerdict::FirstLoss(ChainStage::Dense)
        );
        fixture.daemon.shutdown().await;
    });
}

fn a_retired_object_loses_the_rule_at_eligibility(coverage: &mut Coverage) {
    block_on(async {
        let fixture = Fixture::build().await;
        let healthy = plain(&fixture, &limits(), QUERY);
        let rule = exact_rows(&healthy)[0].clone();
        assert_eq!(
            verdict(&healthy, &rule, ChainStage::Exact),
            StageVerdict::Clean
        );

        fixture.retire("rule").await;
        let run = plain(&fixture, &limits(), QUERY);
        assert!(exact_rows(&run).contains(&rule));
        let report = view(&run).exact_report().unwrap();
        assert!(matches!(
            view(&run).exact.admission,
            ExactAdmission::Judged(_)
        ));
        let judged = report
            .occurrences
            .iter()
            .find(|judged| judged.occurrence_id == rule)
            .unwrap();
        assert!(
            matches!(judged.disposition, Disposition::PolicyExcluded(_)),
            "{judged:?}"
        );
        coverage
            .record("ldg_injection_eligibility_retracted")
            .unwrap();
        assert_eq!(
            verdict(&run, &rule, ChainStage::Exact),
            StageVerdict::FirstLoss(ChainStage::Eligibility)
        );
        fixture.daemon.shutdown().await;
    });
}

fn fused_union_bound_loses_the_rule_at_fusion(coverage: &mut Coverage) {
    block_on(async {
        let fixture = Fixture::build().await;
        let mut bounded = limits();
        bounded.fused_union = NonZeroUsize::MIN;
        let run = plain(&fixture, &bounded, QUERY);
        let rows = exact_rows(&run);
        let rule = rows[0].clone();
        assert_eq!(
            run.ledger.presence(ChainStage::Eligibility, &rule),
            Some(Presence::Reached)
        );
        assert_eq!(
            run.outcome.as_ref().err(),
            Some(&QueryFailure::Unavailable("fused_union"))
        );
        coverage.record("ldg_injection_fusion_union_bound").unwrap();
        assert_eq!(
            verdict(&run, &rule, ChainStage::Exact),
            StageVerdict::FirstLoss(ChainStage::Fusion)
        );
        fixture.daemon.shutdown().await;
    });
}

fn result_rows_bound_loses_the_rule_at_selection(coverage: &mut Coverage) {
    block_on(async {
        let fixture = Fixture::build().await;
        let healthy = plain(&fixture, &limits(), QUERY);
        let fused = fused_ids(&healthy);
        let rule = fused[1].clone();
        assert!(exact_rows(&healthy).contains(&rule), "{fused:?}");

        let mut bounded = limits();
        bounded.result_rows = NonZeroUsize::MIN;
        let run = plain(&fixture, &bounded, QUERY);
        let outcome = outcome(&run);
        assert!(outcome.truncated);
        assert!(fused_ids(&run).contains(&rule));
        let revalidated = outcome.revalidation.as_ref().unwrap();
        assert!(revalidated.occurrences.iter().any(|judged| {
            judged.occurrence_id == rule && judged.disposition == Disposition::Eligible
        }));
        assert!(!entry_ids(&outcome.body).contains(&rule));
        coverage
            .record("ldg_injection_selection_result_rows")
            .unwrap();
        assert_eq!(
            verdict(&run, &rule, ChainStage::Exact),
            StageVerdict::FirstLoss(ChainStage::Selection)
        );
        fixture.daemon.shutdown().await;
    });
}

fn optional_budget_loses_the_rule_at_packing(coverage: &mut Coverage) {
    block_on(async {
        let fixture = Fixture::build().await;
        let mut run = plain(&fixture, &limits(), EXACT_ONLY);
        let fused = fused_ids(&run);
        let rule = fused.last().unwrap().clone();
        let requests = requests(&fixture, &run);
        let Fixture {
            daemon,
            store,
            project,
            projection,
            ..
        } = fixture;
        let (path, lease) = projection.close();
        drop(lease);
        let packer = packing_store(&path);

        let healthy = pack(&packer, &store, &project, &requests, WIDE_TOKENS);
        let admission = healthy.admission.as_ref().unwrap();
        let packed = survivors(admission);
        assert_eq!(packed, fused.iter().cloned().collect::<BTreeSet<_>>());
        let rule_cost = admission
            .admitted()
            .iter()
            .find(|group| {
                group
                    .group
                    .members()
                    .any(|member| member.to_string() == rule)
            })
            .unwrap()
            .cost;
        let spent = WIDE_TOKENS.get() - admission.remaining().get();
        let mut clean = run.ledger.clone();
        observe_packing(&mut clean, Ok(admission));
        assert_eq!(packed_verdict(&clean, &rule), StageVerdict::Clean);

        let injected = pack(
            &packer,
            &store,
            &project,
            &requests,
            ClaudeTokens::new(spent - rule_cost.get()),
        );
        let admission = injected.admission.as_ref().unwrap();
        assert!(
            injected
                .trace
                .read_occurrences()
                .any(|occurrence| occurrence.to_string() == rule),
            "the packer read the rule's row"
        );
        assert!(
            admission.skipped().iter().any(|group| group
                .group
                .members()
                .any(|member| member.to_string() == rule)),
            "{:?}",
            admission.skipped()
        );
        assert!(!admission.admitted().is_empty());
        assert!(!survivors(admission).contains(&rule));
        coverage.record("ldg_injection_packing_skipped").unwrap();
        observe_packing(&mut run.ledger, Ok(admission));
        assert_eq!(
            packed_verdict(&run.ledger, &rule),
            StageVerdict::FirstLoss(ChainStage::Packing)
        );
        drop(packer);
        daemon.shutdown().await;
    });
}

#[test]
fn the_clean_chain_keeps_the_rule_at_every_stage_and_folds_compare_within_one_store() {
    block_on(async {
        let fixture = Fixture::build().await;
        let mut run = plain(&fixture, &limits(), QUERY);
        let rule = exact_rows(&run)[0].clone();
        let requests = requests(&fixture, &run);
        let store_id = fixture
            .store
            .database_incarnation_id_within_budget(&EvalBudget::unbounded())
            .unwrap();
        let Fixture {
            daemon,
            store,
            project,
            projection,
            ..
        } = fixture;
        let (path, lease) = projection.close();
        drop(lease);
        let packer = packing_store(&path);
        let packed = pack(&packer, &store, &project, &requests, WIDE_TOKENS);
        observe_packing(&mut run.ledger, packed.admission.as_ref());
        for stage in eval_core::CHAIN_STAGES {
            let expected = if stage == ChainStage::Dense {
                Presence::NotReached
            } else {
                Presence::Reached
            };
            assert_eq!(
                run.ledger.presence(stage, &rule),
                Some(expected),
                "{stage:?}"
            );
        }
        assert_eq!(run.incarnations.distinct(), 1);
        assert_eq!(run.ledger.incarnations(), BTreeSet::from([0]));
        let clean = Completed {
            verdict: packed_verdict(&run.ledger, &rule),
            database_incarnation_id: store_id.clone(),
        };
        assert_eq!(clean.verdict, StageVerdict::Clean);
        let restarted = Completed {
            verdict: StageVerdict::Clean,
            database_incarnation_id: store_id.clone(),
        };
        assert_eq!(clean.agrees_with(&restarted), Ok(true));
        let other_store = Completed {
            verdict: StageVerdict::Clean,
            database_incarnation_id: format!("{store_id}-other"),
        };
        assert!(matches!(
            clean.agrees_with(&other_store),
            Err(LedgerError::CrossStore { .. })
        ));
        drop(packer);
        daemon.shutdown().await;
    });
}

#[test]
fn reports_from_two_stores_carry_two_incarnations_and_fold_indeterminate() {
    block_on(async {
        let first = Fixture::build().await;
        let second = Fixture::build().await;
        let mut incarnations = Incarnations::default();
        let one = plain(&first, &limits(), EXACT_ONLY);
        let rule = exact_rows(&one)[0].clone();
        let mut ledger = ChainLedger::default();
        observe_lanes(&mut ledger, view(&one), &mut incarnations);
        let other = plain(&second, &limits(), EXACT_ONLY);
        let report = view(&other).exact_report().unwrap();
        let token = incarnations.token(report.incarnation);
        ledger.observe(
            eval_core::Observation::new(
                ChainStage::Eligibility,
                1,
                Some(token),
                exact_rows(&other).into_iter().collect(),
            )
            .unwrap(),
        );
        assert_eq!(
            incarnations.distinct(),
            2,
            "two kernel stores judge under two incarnations"
        );
        assert_eq!(ledger.incarnations(), BTreeSet::from([0, 1]));
        assert_eq!(
            ledger.verdict(
                &required(&rule, ChainStage::Exact),
                &BTreeSet::new(),
                ChainStage::Eligibility
            ),
            StageVerdict::Indeterminate
        );
        first.daemon.shutdown().await;
        second.daemon.shutdown().await;
    });
}

fn dense_ready<'a>(query: &'a [f32], producer: &'a ExhaustiveProducer) -> DenseLane<'a> {
    DenseLane::Ready {
        query,
        generation_id: GENERATION,
        producer,
    }
}

#[test]
fn a_held_classification_window_makes_eligibility_unjoinable_never_absent() {
    block_on(async {
        let fixture = Fixture::build().await;
        fixture.store_vectors(vector_for);
        let query = query_vector();
        let mut limits = limits();
        limits.dense = Some(dense_limits(8));
        let producer = ExhaustiveProducer {
            limits: dense_limits(8),
        };
        let healthy = drive(
            &fixture,
            &limits,
            QUERY,
            dense_ready(&query, &producer),
            |_| {},
        );
        let rule = exact_rows(&healthy)[0].clone();
        assert_eq!(
            verdict(&healthy, &rule, ChainStage::Exact),
            StageVerdict::Clean
        );

        // The dense lane judges under the projection connection, before the
        // admission phase, so it alone keeps a ranking once the window opens.
        let mut window = None;
        let run = drive(
            &fixture,
            &limits,
            QUERY,
            dense_ready(&query, &producer),
            |phase| {
                if phase == Phase::Admission {
                    window = Some(fixture.store.hold_classification_change_for_test());
                }
            },
        );
        drop(window);
        let view = view(&run);
        assert_eq!(
            *view.status(Lane::Exact),
            LaneStatus::Unavailable("snapshot_changed")
        );
        assert_eq!(
            *view.status(Lane::Lexical),
            LaneStatus::Unavailable("snapshot_changed")
        );
        assert_eq!(*view.status(Lane::Dense), LaneStatus::Complete);
        let ExactAdmission::Moved(report) = &view.exact.admission else {
            panic!("{:?}", view.exact.admission);
        };
        assert!(!report.is_reusable());
        assert!(
            report
                .occurrences
                .iter()
                .any(|judged| judged.occurrence_id == rule)
        );
        assert!(exact_rows(&run).contains(&rule));
        assert_eq!(
            run.ledger.presence(ChainStage::Exact, &rule),
            Some(Presence::Reached)
        );
        assert_eq!(run.ledger.presence(ChainStage::Eligibility, &rule), None);
        assert_eq!(run.ledger.presence(ChainStage::Lexical, &rule), None);
        assert_eq!(
            run.outcome.as_ref().err(),
            Some(&QueryFailure::Unavailable("snapshot_changed")),
            "revalidation under the same window refuses the request"
        );
        assert_eq!(run.ledger.presence(ChainStage::Fusion, &rule), None);
        assert_eq!(
            verdict(&run, &rule, ChainStage::Exact),
            StageVerdict::Indeterminate,
            "a loss behind the unjoinable window is never placed at eligibility"
        );
        fixture.daemon.shutdown().await;
    });
}

#[test]
fn a_window_opened_at_fusion_makes_revalidation_unjoinable_not_a_selection_loss() {
    block_on(async {
        let fixture = Fixture::build().await;
        let healthy = plain(&fixture, &limits(), QUERY);
        let rule = exact_rows(&healthy)[0].clone();
        let mut window = None;
        let run = drive(&fixture, &limits(), QUERY, DenseLane::Undeclared, |phase| {
            if phase == Phase::Fusion {
                window = Some(fixture.store.hold_classification_change_for_test());
            }
        });
        drop(window);
        assert!(matches!(
            view(&run).exact.admission,
            ExactAdmission::Judged(_)
        ));
        assert_eq!(
            run.ledger.presence(ChainStage::Eligibility, &rule),
            Some(Presence::Reached)
        );
        assert_eq!(
            run.outcome.as_ref().err(),
            Some(&QueryFailure::Unavailable("snapshot_changed"))
        );
        assert_eq!(run.ledger.presence(ChainStage::Fusion, &rule), None);
        assert_eq!(
            run.ledger.presence(ChainStage::Selection, &rule),
            Some(Presence::NotReached)
        );
        assert_eq!(
            verdict(&run, &rule, ChainStage::Exact),
            StageVerdict::Indeterminate
        );
        fixture.daemon.shutdown().await;
    });
}

/// Admission returns the rows an unavailable exact lane read but no verdicts;
/// the shell must not read that as the kernel excluding them.
#[test]
fn an_exact_lane_that_ends_after_reading_leaves_eligibility_unjoinable() {
    block_on(async {
        let fixture = Fixture::build().await;
        let healthy = plain(&fixture, &limits(), QUERY);
        let mut view = view(&healthy).clone();
        let rule = view
            .exact
            .rows
            .iter()
            .map(|row| row.occurrence_id.clone())
            .find(|id| !view.ranking(Lane::Lexical).contains(id))
            .expect("an exact-only row exists");
        view.statuses[0] = LaneStatus::Unavailable("kernel");
        view.exact = ExactReport {
            rows: view.exact.rows.clone(),
            admission: ExactAdmission::KernelError,
        };
        let mut ledger = ChainLedger::default();
        observe_lanes(&mut ledger, &view, &mut Incarnations::default());
        assert_eq!(
            ledger.presence(ChainStage::Exact, &rule),
            Some(Presence::Reached)
        );
        assert_eq!(ledger.presence(ChainStage::Eligibility, &rule), None);

        let mut unread = view.clone();
        unread.statuses[0] = LaneStatus::Unavailable("identity");
        unread.exact = ExactReport {
            rows: Vec::new(),
            admission: ExactAdmission::NotJudged,
        };
        let mut ledger = ChainLedger::default();
        observe_lanes(&mut ledger, &unread, &mut Incarnations::default());
        assert_eq!(ledger.presence(ChainStage::Exact, &rule), None);
        assert_eq!(
            ledger.presence(ChainStage::Eligibility, &rule),
            Some(Presence::ReachedEvidenceAbsent),
            "the other lanes' admission still joins"
        );
        assert_eq!(
            ledger.verdict(
                &required(&rule, ChainStage::Exact),
                &BTreeSet::new(),
                ChainStage::Eligibility
            ),
            StageVerdict::Indeterminate
        );
        fixture.daemon.shutdown().await;
    });
}

#[test]
fn tap_returns_leave_production_output_byte_equal() {
    block_on(async {
        let fixture = Fixture::build().await;
        let (_token, budget) = request_budget(10_000);
        let direct = execute(
            &fixture.projection,
            &fixture.store,
            Authority {
                project: &fixture.project,
                destination: ArtifactDestination::Local,
            },
            &limits(),
            budget.shared(),
            QUERY,
            DenseLane::Undeclared,
            |_| {},
        )
        .unwrap();
        let observed = plain(&fixture, &limits(), QUERY);
        assert!(observed.ledger.observations().count() >= 5);
        let shell = outcome(&observed);
        assert_eq!(
            serde_json::to_vec(&direct.body).unwrap(),
            serde_json::to_vec(&shell.body).unwrap()
        );
        assert_eq!(direct.statuses, shell.statuses);
        assert_eq!(direct.fused, shell.fused);
        assert_eq!(direct.exact, shell.exact);
        assert_eq!(direct.revalidation, shell.revalidation);

        let requests = requests(&fixture, &observed);
        let Fixture {
            daemon,
            store,
            project,
            projection,
            ..
        } = fixture;
        let (path, lease) = projection.close();
        drop(lease);
        let packer = packing_store(&path);
        let first = pack(&packer, &store, &project, &requests, WIDE_TOKENS);
        let second = pack(&packer, &store, &project, &requests, WIDE_TOKENS);
        let read: Vec<_> = first.trace.read_occurrences().collect();
        assert_eq!(read.len(), requests.len());
        assert_eq!(first.admission.unwrap(), second.admission.unwrap());
        drop(packer);
        daemon.shutdown().await;
    });
}

#[test]
fn a_misattributed_observation_and_missing_coverage_fail_the_self_test() {
    block_on(async {
        let fixture = Fixture::build().await;
        let healthy = plain(&fixture, &limits(), QUERY);
        let rule = fused_ids(&healthy)[1].clone();
        let mut bounded = limits();
        bounded.result_rows = NonZeroUsize::MIN;
        let run = plain(&fixture, &bounded, QUERY);
        let expected = StageVerdict::FirstLoss(ChainStage::Selection);
        assert_eq!(verdict(&run, &rule, ChainStage::Exact), expected);

        let mut misattributed = ChainLedger::default();
        for observation in run.ledger.observations() {
            let stage = match observation.stage() {
                ChainStage::Fusion => ChainStage::Selection,
                ChainStage::Selection => ChainStage::Fusion,
                other => other,
            };
            misattributed.observe(observation.at(stage));
        }
        assert_eq!(
            misattributed.verdict(
                &required(&rule, ChainStage::Exact),
                &BTreeSet::new(),
                ChainStage::Selection
            ),
            StageVerdict::FirstLoss(ChainStage::Fusion),
            "a shell that swaps two stage labels names the wrong stage and fails the self-test"
        );
        fixture.daemon.shutdown().await;
    });

    let owned: Vec<&str> = MARKERS
        .iter()
        .filter(|marker| marker.test.starts_with(SUITE))
        .map(|marker| marker.name)
        .collect();
    let mut coverage = Coverage::default();
    for name in &owned[1..] {
        coverage.record(name).unwrap();
    }
    assert_eq!(
        coverage.complete(SUITE),
        Err(CoverageError::Incomplete {
            missing: BTreeSet::from([owned[0]]),
        })
    );
}

type Scenario = fn(&mut Coverage);

fn scenarios() -> [(&'static str, Scenario); 7] {
    [
        (
            "exact_page_bound_loses_the_rule_at_the_exact_lane",
            exact_page_bound_loses_the_rule_at_the_exact_lane,
        ),
        (
            "lexical_accepted_bound_loses_the_rule_at_the_lexical_lane",
            lexical_accepted_bound_loses_the_rule_at_the_lexical_lane,
        ),
        (
            "dense_k_bound_loses_the_rule_at_the_dense_lane",
            dense_k_bound_loses_the_rule_at_the_dense_lane,
        ),
        (
            "a_retired_object_loses_the_rule_at_eligibility",
            a_retired_object_loses_the_rule_at_eligibility,
        ),
        (
            "fused_union_bound_loses_the_rule_at_fusion",
            fused_union_bound_loses_the_rule_at_fusion,
        ),
        (
            "result_rows_bound_loses_the_rule_at_selection",
            result_rows_bound_loses_the_rule_at_selection,
        ),
        (
            "optional_budget_loses_the_rule_at_packing",
            optional_budget_loses_the_rule_at_packing,
        ),
    ]
}

fn run(name: &str) {
    let (_, scenario) = scenarios().into_iter().find(|(n, _)| *n == name).unwrap();
    let mut coverage = Coverage::default();
    scenario(&mut coverage);
    let marker = MARKERS
        .iter()
        .find(|m| m.test == format!("{SUITE}{name}"))
        .unwrap();
    assert!(
        coverage.fired().contains(marker.name),
        "{name} records its marker"
    );
}

#[test]
fn packing_requests_are_the_entries_selection_kept() {
    block_on(async {
        let fixture = Fixture::build().await;
        let mut bounded = limits();
        bounded.result_rows = NonZeroUsize::MIN;
        let run = plain(&fixture, &bounded, QUERY);
        let outcome = outcome(&run);
        assert!(outcome.truncated);
        assert!(fused_ids(&run).len() > 1, "the cap cut the fused set");
        let requested: Vec<String> = requests(&fixture, &run)
            .iter()
            .map(|request| request.occurrence.to_string())
            .collect();
        assert_eq!(
            requested,
            entry_ids(&outcome.body),
            "packing receives only what selection kept"
        );
        fixture.daemon.shutdown().await;
    });
}

#[test]
fn a_packing_refusal_is_a_loss_at_a_bound_and_unjoinable_at_a_fault() {
    let bounds = [
        PreparationRefusal::OptionalBound(BoundExceeded {
            bound: OptionalBound::FusedCandidates,
            at: 16,
        }),
        PreparationRefusal::CloseOverBudget {
            limit: ClaudeTokens::new(1),
            charged: ClaudeTokens::new(2),
        },
    ];
    for refusal in &bounds {
        let mut ledger = ChainLedger::default();
        observe_packing(&mut ledger, Err(refusal));
        assert_eq!(
            ledger.presence(ChainStage::Packing, "rule"),
            Some(Presence::ReachedEvidenceAbsent),
            "a bound ends the request with nothing packed: {refusal:?}"
        );
    }
    let faults = [
        PreparationRefusal::Deadline,
        PreparationRefusal::Storage("busy".to_string()),
        PreparationRefusal::Kernel(kernel::KernelError::Io),
    ];
    for refusal in &faults {
        let mut ledger = ChainLedger::default();
        observe_packing(&mut ledger, Err(refusal));
        assert_eq!(
            ledger.presence(ChainStage::Packing, "rule"),
            None,
            "a fault leaves no closed packing result: {refusal:?}"
        );
    }
}

#[test]
fn over_bound_stage_returns_are_unjoinable_instead_of_panicking() {
    block_on(async {
        let fixture = Fixture::build().await;
        let run = plain(&fixture, &limits(), QUERY);
        let mut view = view(&run).clone();
        let lexical = Lane::ORDER
            .iter()
            .position(|lane| *lane == Lane::Lexical)
            .unwrap();
        view.rankings[lexical] = (0..=MAX_CANDIDATES_PER_STAGE_OBSERVATION)
            .map(|index| format!("over-bound-{index}"))
            .collect();
        let mut ledger = ChainLedger::default();
        observe_lanes(&mut ledger, &view, &mut Incarnations::default());
        assert_eq!(ledger.presence(ChainStage::Lexical, "over-bound-0"), None);
        assert_eq!(
            ledger.presence(ChainStage::Eligibility, "over-bound-0"),
            None
        );
        fixture.daemon.shutdown().await;
    });
}

#[test]
fn exact_injection() {
    run("exact_page_bound_loses_the_rule_at_the_exact_lane");
}

#[test]
fn lexical_injection() {
    run("lexical_accepted_bound_loses_the_rule_at_the_lexical_lane");
}

#[test]
fn dense_injection() {
    run("dense_k_bound_loses_the_rule_at_the_dense_lane");
}

#[test]
fn eligibility_injection() {
    run("a_retired_object_loses_the_rule_at_eligibility");
}

#[test]
fn fusion_injection() {
    run("fused_union_bound_loses_the_rule_at_fusion");
}

#[test]
fn selection_injection() {
    run("result_rows_bound_loses_the_rule_at_selection");
}

#[test]
fn packing_injection() {
    run("optional_budget_loses_the_rule_at_packing");
}

#[test]
fn ledger_markers_each_name_a_scenario_here() {
    let scenario_names: BTreeSet<&str> = scenarios().iter().map(|(n, _)| *n).collect();
    let owned: Vec<&str> = MARKERS
        .iter()
        .filter(|m| m.name.starts_with("ldg_"))
        .map(|m| {
            m.test
                .strip_prefix(SUITE)
                .unwrap_or_else(|| panic!("{}", m.test))
        })
        .collect();
    assert_eq!(owned.len(), scenario_names.len());
    for test in owned {
        assert!(scenario_names.contains(test), "{test}");
    }
}

/// The completeness proof: one run of every scenario fires every marker this
/// suite owns.
#[test]
fn every_ledger_marker_fires_across_the_scenarios() {
    let mut coverage = Coverage::default();
    for (_, scenario) in scenarios() {
        scenario(&mut coverage);
    }
    coverage.complete(SUITE).unwrap();
}

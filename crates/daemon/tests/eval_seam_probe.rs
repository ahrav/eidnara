//! Every seam the evaluator requires, named once so the daemon test build
//! (`--all-features`, matching CI) fails to compile when one moves or loses
//! its feature gate. Retrieval's test-support seams are reachable only because
//! the daemon's dev-dependencies enable that feature.

#![cfg(feature = "test-support")]

use daemon::embedding_publication::EmbeddingPublisher;
use daemon::git_sources::read_selection;
use daemon::harness_sources::{MAX_REVISION_LEAD_MS, SourcePublisher, opencode_units, pi_units};
use daemon::packing::PackingTrace;
use daemon::query_route::{Admitted, ExactAdmission, ExactReport, admit_lanes, execute, select};
use daemon::search_catchup::SearchCatchUp;
use daemon::transform::UserHintOutcome;
use daemon::{Handler, HandlerCore};
use eval_core::{Stage, Surface1Stage};
use kernel::KernelStore;
use retrieval::batch::{BatchFault, apply_batch_with_fault_for_test};
use retrieval::persist_occurrences_with_digests_for_test;

/// Naming each item is the check; the body exists so the probe is a test.
#[test]
fn every_evaluator_seam_is_reachable_under_all_features() {
    let _ = opencode_units;
    let _ = pi_units;
    let _ = SourcePublisher::publish;
    let _ = read_selection;
    let _ = KernelStore::judge_surface_eligibility;
    let _ = KernelStore::egress_candidates;
    let _ = EmbeddingPublisher::publish;
    let _ = SearchCatchUp::run_episode;
    let _ = apply_batch_with_fault_for_test;
    let _ = persist_occurrences_with_digests_for_test;
    let _ = [BatchFault::AfterAdmission, BatchFault::AfterPending];
    let _ = KernelStore::hold_classification_change_for_test;
    let _ = KernelStore::database_incarnation_id_within_budget;
    let _ = PackingTrace::read_occurrences;
    let _ = |admitted: &Admitted<'_>| {
        (
            admitted.statuses.len(),
            admitted.lanes.rankings().count(),
            admitted.exact.rows.len(),
        )
    };
    let _ = |report: &ExactReport| match &report.admission {
        ExactAdmission::Judged(report) | ExactAdmission::Moved(report) => report.is_reusable(),
        ExactAdmission::NotJudged | ExactAdmission::KernelError => report.rows.is_empty(),
    };
    assert_eq!(
        kernel::MAX_ELIGIBILITY_CANDIDATES,
        eval_core::MAX_CANDIDATES_PER_STAGE_OBSERVATION
    );
    let _ = Handler::core_for_test;
    let _ = HandlerCore::user_hint_outcome_for_test;
    let _ = |outcome: &UserHintOutcome| {
        (
            outcome.trace.selected.len(),
            outcome.deferred,
            outcome.applied,
            outcome.attached,
        )
    };
    assert_eq!(
        <Surface1Stage as Stage>::ALL.len(),
        eval_core::SURFACE1_STAGES.len()
    );
    assert_eq!(
        eval_core::table_digest(),
        eval_core::FAILURE_CLASS_TABLE_DIGEST
    );
    // `execute` and its halves take an `impl FnMut`, so a non-executed call names each.
    let _ = |p, k, a, l, b, q, d| execute(p, k, a, l, b, q, d, |_| {});
    let _ = |p, k, a, l, b, q, d| admit_lanes(p, k, a, l, b, q, d, |_| {});
    let _ = |admitted| select(admitted, |_| {});
    assert_eq!(MAX_REVISION_LEAD_MS, eval_core::MAX_REVISION_LEAD_MS);
}

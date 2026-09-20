//! Every seam the evaluator requires, named once so the daemon test build
//! (`--all-features`, matching CI) fails to compile when one moves or loses
//! its feature gate. Retrieval's test-support seams are reachable only because
//! the daemon's dev-dependencies enable that feature.

#![cfg(feature = "test-support")]

use daemon::embedding_publication::EmbeddingPublisher;
use daemon::git_sources::read_selection;
use daemon::harness_sources::{MAX_REVISION_LEAD_MS, SourcePublisher, opencode_units, pi_units};
use daemon::query_route::execute;
use daemon::search_catchup::SearchCatchUp;
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
    // `execute` takes an `impl FnMut`, so a non-executed call names it.
    let _ = |p, k, a, l, b, q, d| execute(p, k, a, l, b, q, d, |_| {});
    assert_eq!(MAX_REVISION_LEAD_MS, eval_core::MAX_REVISION_LEAD_MS);
}

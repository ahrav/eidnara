#![cfg(all(unix, feature = "test-support"))]

mod support;

#[path = "../examples/eval_runner/aging.rs"]
#[allow(dead_code)]
mod aging;
#[path = "../examples/eval_runner/campaign.rs"]
#[allow(dead_code)]
mod campaign;

use aging::{Full, Plan, RunError, Stores, full_life, live, plan};
use campaign::Charges;
use eval_core::{
    ConstructionKind, Divergence, GuardComparison, PrefixRefused, Scale, StateSnapshot,
    StoreFamily, WindowDeaths,
};
use support::embedding_fixtures::{batch_bounds, hold_admission};

const MESSAGES: u32 = 40;
const LONG_MESSAGES: u32 = 100;

fn charges() -> Charges {
    let profile = campaign::profile(Scale::S0, 128, 600_000, None);
    Charges::new(profile.envelope)
}

fn straddling_plan() -> Plan {
    let plan = plan(MESSAGES).unwrap();
    assert!(plan.checkpoint_step > 0);
    assert!((plan.checkpoint_step as usize) < plan.steps.len());
    plan
}

#[test]
fn the_aged_arm_is_built_by_replay_and_matches_the_bulk_scaffold_only_by_enumerated_deaths() {
    let plan = straddling_plan();
    let mut charges = charges();
    let full: Full = full_life(&plan, &mut charges).unwrap();
    assert_eq!(full.incarnation_id.len(), 32);
    assert!(full.state.commit_seq > 0);
    assert_eq!(full.against_bulk.earlier.kind, ConstructionKind::CatchUp);
    assert_eq!(full.against_bulk.later.kind, ConstructionKind::Bulk);
    assert!(
        full.against_bulk.earlier.snapshot_commit_seq < full.against_bulk.later.snapshot_commit_seq
    );
    assert!(full.against_bulk.live_digests_equal);
    assert!(full.rows.live.pending_embedding.is_empty());
    assert!(
        full.against_bulk
            .divergences
            .iter()
            .any(|d| matches!(d, Divergence::TombstonedBeforeSnapshot { .. }))
    );
    let root = tempfile::tempdir().unwrap();
    let mut prefix = Stores::open(root.path(), &plan);
    live(&mut prefix, &plan.steps[..plan.checkpoint_step as usize]);
    let checkpoint_tip = prefix.tip();
    assert!(checkpoint_tip < full.state.commit_seq);
    let window = WindowDeaths::count(&full.state.kernel, checkpoint_tip, full.state.commit_seq);
    assert!(window.supersessions > 0);
    assert!(window.retirements > 0);
    assert!(full.state.kernel.values().any(|d| {
        d.invalidated_commit_seq
            .is_some_and(|at| at <= checkpoint_tip)
    }));
}

#[test]
fn two_lives_of_one_history_share_a_guard_digest_and_a_slipped_family_is_named() {
    let plan = straddling_plan();
    let mut charges = charges();
    let first = full_life(&plan, &mut charges).unwrap();
    let second = full_life(&plan, &mut charges).unwrap();
    assert_ne!(first.incarnation_id, second.incarnation_id);
    StateSnapshot::compare(&first.state, &second.state).unwrap();
    assert_eq!(
        first.state.guard_digest().unwrap(),
        second.state.guard_digest().unwrap()
    );
    let both = GuardComparison::of(
        (&first.rows, ConstructionKind::CatchUp),
        (&second.rows, ConstructionKind::CatchUp),
    )
    .unwrap();
    assert!(both.live_digests_equal);
    assert!(both.divergences.is_empty());

    let root = tempfile::tempdir().unwrap();
    let mut short = Stores::open(root.path(), &plan);
    live(&mut short, &plan.steps[..plan.checkpoint_step as usize]);
    let prefix = short.snapshot();
    assert_eq!(
        StateSnapshot::compare(&first.state, &prefix),
        Err(PrefixRefused::CommitSeqDiffers {
            full: first.state.commit_seq,
            resumed: prefix.commit_seq,
        })
    );
    let mut slipped = first.state.clone();
    slipped.memory.clear();
    assert_eq!(
        StateSnapshot::compare(&first.state, &slipped),
        Err(PrefixRefused::HistorySlipped {
            family: StoreFamily::Memory,
        })
    );
}

#[test]
fn a_history_beyond_the_fixture_bounds_is_lived_and_matches_the_bulk_scaffold() {
    let plan = plan(LONG_MESSAGES).unwrap();
    assert!(plan.bounds.hold.admission.max_references > hold_admission().max_references);
    let mut charges = charges();
    let full = full_life(&plan, &mut charges).unwrap();
    assert!(full.rows.live.occurrences.len() > batch_bounds().max_local_mutations.get());
    assert!(full.against_bulk.live_digests_equal);
    assert!(full.rows.live.pending_embedding.is_empty());
}

#[test]
fn a_history_too_short_to_straddle_a_death_is_refused() {
    assert!(matches!(plan(2), Err(RunError::NoStraddlingStep)));
}

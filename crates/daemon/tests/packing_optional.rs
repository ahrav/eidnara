//! The optional phase runs after the required phase over the budget it left,
//! groups by key, and admits groups by skip-and-continue in fused order.

mod support;

use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use daemon::packing::{
    AccountingProfile, BLOCK_CLOSE_FRAGMENT, Charged, ClaudeTokens, OptionalAdmission,
    OptionalExclusion, OptionalRequest, PackingTrace, PreparationRefusal, RequiredInputs,
    StageEvent, prepare_optional, prepare_required, render,
};
use kernel::EligibilityVerdict;
use kernel::applicability::EvalBudget;
use retrieval::fusion::OccurrenceId;
use retrieval::packing::{OptionalBound, OptionalBounds, RequiredContextFailure};
use retrieval::{Tombstone, TombstoneReason, tombstone_occurrence};
use support::packing::{
    Fixture, ToolSpan, accounting_bounds, bounds, byte_profile, required_render_total, tool_range,
    tool_span, while_connection_is_held,
};

const REQUIRED: ToolSpan = tool_span("req", "1", "required bytes\n");
const PARENT: &str = "0123456789abcdefghij";

fn wide() -> OptionalBounds {
    OptionalBounds {
        max_fused_candidates: NonZeroUsize::new(16).unwrap(),
        max_parents: NonZeroUsize::new(16).unwrap(),
        max_spans_per_parent: NonZeroUsize::new(16).unwrap(),
        max_payload_loads: NonZeroUsize::new(16).unwrap(),
        max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_item_bytes: NonZeroU64::new(1 << 19).unwrap(),
    }
}

fn optional(span: &ToolSpan) -> OptionalRequest {
    OptionalRequest {
        occurrence: span.id(),
        revision: span.revision.parse().unwrap(),
    }
}

fn run(
    fixture: &Fixture,
    optional_requests: &[OptionalRequest],
    optional_bounds: &OptionalBounds,
    budget: u64,
) -> (Result<OptionalAdmission, PreparationRefusal>, PackingTrace) {
    run_with_profile(
        fixture,
        optional_requests,
        optional_bounds,
        budget,
        &byte_profile(),
    )
}

fn run_with_profile(
    fixture: &Fixture,
    optional_requests: &[OptionalRequest],
    optional_bounds: &OptionalBounds,
    budget: u64,
    profile: &AccountingProfile,
) -> (Result<OptionalAdmission, PreparationRefusal>, PackingTrace) {
    let mut trace = PackingTrace::default();
    trace.note_retrieval_call();
    let eval = EvalBudget::unbounded();
    let inputs = RequiredInputs {
        kernel: &fixture.kernel,
        project: &fixture.project,
        destination: kernel::ArtifactDestination::Local,
        budget: &eval,
        profile,
    };
    let required = prepare_required(
        &fixture.store,
        inputs,
        &[REQUIRED.request()],
        &bounds(budget),
        &accounting_bounds(),
        &mut trace,
    )
    .unwrap();
    let result = prepare_optional(
        &fixture.store,
        inputs,
        &required,
        optional_requests,
        optional_bounds,
        &accounting_bounds(),
        &mut trace,
    );
    (result, trace)
}

fn optional_starts_after_the_last_required_event(trace: &PackingTrace) {
    let last_required = trace
        .events()
        .iter()
        .rposition(|event| matches!(event, StageEvent::Required(..)))
        .unwrap();
    let first_optional = trace
        .events()
        .iter()
        .position(|event| matches!(event, StageEvent::Optional(..)))
        .unwrap();
    assert!(last_required < first_optional, "{:?}", trace.events());
    assert_eq!(trace.retrieval_calls(), 1);
}

#[test]
fn optional_groups_are_admitted_by_skip_and_continue_over_the_remaining_budget() {
    let eleven = tool_span(
        "opt-a",
        "1",
        "a group whose rendered cost alone exceeds what the other two cost together, \
         so the scan must skip it and continue to the smaller groups behind it",
    );
    let four = tool_span("opt-b", "1", "four");
    let six = tool_span("opt-c", "1", "sixsix");
    let fixture = Fixture::new(&[REQUIRED, eleven, four, six]);
    let requests = [optional(&eleven), optional(&four), optional(&six)];
    let required_charge = required_render_total(&[REQUIRED]);

    let (generous, _) = run(&fixture, &requests, &wide(), 1 << 20);
    let generous = generous.unwrap();
    let cost_of = |first_fused: usize| {
        generous
            .admitted()
            .iter()
            .find(|group| group.group.first_fused == first_fused)
            .map(|group| group.cost.get())
            .unwrap()
    };
    let (big, small, medium) = (cost_of(0), cost_of(1), cost_of(2));
    assert!(small < medium && medium < big, "{small} {medium} {big}");
    for group in generous.admitted() {
        assert_eq!(
            group.cost,
            ClaudeTokens::new(render::group_fragment(&group.group).len() as u64),
            "a group's cost is its rendered delta under the byte profile"
        );
    }
    assert_eq!(
        generous
            .ledger()
            .entries()
            .iter()
            .map(|entry| entry.bytes)
            .sum::<usize>(),
        generous.ledger().text().len()
    );

    let remaining = small + medium;
    assert!(big > remaining, "the first group alone must not fit");
    // The budget covers the closed render, so the block close is part of it.
    let close = render::BLOCK_CLOSE_FRAGMENT.len() as u64;
    let (result, trace) = run(
        &fixture,
        &requests,
        &wide(),
        required_charge + remaining + close,
    );
    let admission = result.unwrap();
    let admitted: Vec<_> = admission
        .admitted()
        .iter()
        .map(|group| (group.group.first_fused, group.cost.get()))
        .collect();
    assert_eq!(admitted, vec![(1, small), (2, medium)]);
    assert_eq!(admission.skipped().len(), 1);
    assert_eq!(admission.skipped()[0].group.first_fused, 0);
    assert_eq!(admission.skipped()[0].cost, ClaudeTokens::new(big));
    assert_eq!(admission.remaining(), ClaudeTokens::new(0));
    assert!(admission.excluded().is_empty());
    assert!(admission.ungrouped().is_empty());
    assert_eq!(trace.payload_loads(), 4);
    optional_starts_after_the_last_required_event(&trace);
    let text = admission.ledger().text();
    assert!(text.starts_with("<packed-context>\n<required"));
    assert!(text.ends_with("</group>\n</packed-context>\n"));
    assert!(
        !text.contains("elevenbytes"),
        "a skipped group is not rendered"
    );
    assert!(text.contains("four") && text.contains("sixsix"));

    let (spare, _) = run(
        &fixture,
        &requests[1..],
        &wide(),
        required_charge + remaining + close + 3,
    );
    assert_eq!(
        spare.unwrap().remaining(),
        ClaudeTokens::new(3),
        "unused budget is success"
    );
}

#[test]
fn same_parent_spans_group_and_are_charged_as_one_merged_range() {
    let a = tool_range("tool", PARENT, 0, 6);
    let b = tool_range("tool", PARENT, 4, 10);
    let c = tool_range("tool", PARENT, 15, 20);
    let other = tool_span("other", "1", "zz");
    let fixture = Fixture::new(&[REQUIRED, a, b, c, other]);
    let requests = [optional(&other), optional(&a), optional(&b), optional(&c)];
    let (result, _) = run(&fixture, &requests, &wide(), 1 << 20);
    let admission = result.unwrap();
    assert_eq!(admission.admitted().len(), 2);
    assert_eq!(admission.admitted()[0].group.first_fused, 0);
    let grouped = &admission.admitted()[1];
    assert_eq!(grouped.group.first_fused, 1);
    assert_eq!(grouped.group.ranges.len(), 2);
    assert_eq!(grouped.group.ranges[0].bytes, PARENT.as_bytes()[0..10]);
    assert_eq!(grouped.group.ranges[1].bytes, PARENT.as_bytes()[15..20]);
    assert_eq!(
        grouped.cost,
        ClaudeTokens::new(render::group_fragment(&grouped.group).len() as u64)
    );
    let wrapper_entries: Vec<_> = admission
        .ledger()
        .entries()
        .iter()
        .filter(|entry| matches!(entry.item, Charged::GroupOpen(1) | Charged::GroupClose(1)))
        .collect();
    assert_eq!(
        wrapper_entries.len(),
        2,
        "the group wrapper is charged once, as its own entries"
    );
    let range_entries = admission
        .ledger()
        .entries()
        .iter()
        .filter(|entry| matches!(entry.item, Charged::Range(1, _)))
        .count();
    assert_eq!(range_entries, 2);
}

/// The scan deducts each group's priced cost from the budget; the ledger
/// charges the group's entries. The two must agree under a profile whose
/// headroom rounds per charge, and the budget must cover the closed render.
#[test]
fn a_groups_priced_cost_equals_its_charged_entries_and_the_budget_covers_the_closed_render() {
    let a = tool_range("tool", PARENT, 0, 6);
    let b = tool_range("tool", PARENT, 4, 10);
    let c = tool_range("tool", PARENT, 15, 20);
    let other = tool_span("other", "1", "zz");
    let fixture = Fixture::new(&[REQUIRED, a, b, c, other]);
    let requests = [optional(&other), optional(&a), optional(&b), optional(&c)];
    let headroom = AccountingProfile::heuristic("bytes-over-four", "bytes/4", 250, |text| {
        text.len().div_ceil(4)
    });
    for profile in [
        byte_profile(),
        headroom,
        AccountingProfile::exact_tokenizer(),
    ] {
        let limit = 1 << 20;
        let (result, _) = run_with_profile(&fixture, &requests, &wide(), limit, &profile);
        let admission = result.unwrap();
        assert_eq!(admission.admitted().len(), 2, "{}", profile.identity());
        for (index, group) in admission.admitted().iter().enumerate() {
            let charged = admission
                .ledger()
                .entries()
                .iter()
                .filter(|entry| match entry.item {
                    Charged::GroupOpen(i) | Charged::Range(i, _) | Charged::GroupClose(i) => {
                        i == index
                    }
                    _ => false,
                })
                .map(|entry| entry.charge.with_headroom().get())
                .sum::<u64>();
            assert_eq!(
                group.cost.get(),
                charged,
                "{}: group {index} priced {} but charged {charged}",
                profile.identity(),
                group.cost.get()
            );
        }
        assert_eq!(
            limit - admission.remaining().get(),
            admission.ledger().total_with_headroom().get(),
            "{}: the budget consumed equals the closed ledger's total",
            profile.identity()
        );
    }
}

/// A heuristic whose count is not monotonic in the render: an odd number of
/// rendered spans counts as more than half the `u64` range, an even number as
/// nothing. The required render has no span and costs nothing; a group of
/// three ranges is priced at two deltas above half the range, whose sum is
/// unrepresentable.
fn half_plus_one_on_odd_spans() -> AccountingProfile {
    AccountingProfile::heuristic("half-plus-one", "odd spans", 0, |text| {
        if text.matches("<span").count() % 2 == 1 {
            (u64::MAX / 2 + 1) as usize
        } else {
            0
        }
    })
}

#[test]
fn a_group_whose_cost_overflows_is_refused_not_admitted_at_a_saturated_cost() {
    let a = tool_range("tool", PARENT, 0, 6);
    let b = tool_range("tool", PARENT, 8, 12);
    let c = tool_range("tool", PARENT, 15, 20);
    let fixture = Fixture::new(&[REQUIRED, a, b, c]);
    let requests = [optional(&a), optional(&b), optional(&c)];
    let (result, _) = run_with_profile(
        &fixture,
        &requests,
        &wide(),
        u64::MAX,
        &half_plus_one_on_odd_spans(),
    );
    match result {
        Err(PreparationRefusal::OptionalCostOverflow { at }) => assert_eq!(at, 0),
        other => panic!("an unrepresentable cost must not be admitted: {other:?}"),
    }
}

/// The required phase reserves the block open and the required fragments;
/// the optional phase must still close the block. A limit the required render
/// meets exactly leaves no room for the close, so the optional phase refuses
/// rather than returning a render the budget does not cover.
#[test]
fn a_required_render_that_leaves_no_room_for_the_close_refuses_the_optional_phase() {
    let a = tool_span("opt-a", "1", "aaaa");
    let fixture = Fixture::new(&[REQUIRED, a]);
    let limit = required_render_total(&[REQUIRED]);
    for requests in [&[][..], &[optional(&a)][..]] {
        let (result, _) = run(&fixture, requests, &wide(), limit);
        match result {
            Err(PreparationRefusal::Required(RequiredContextFailure::OverBudget {
                limit: reported,
                charged,
            })) => {
                assert_eq!(reported, ClaudeTokens::new(limit));
                assert_eq!(
                    charged,
                    ClaudeTokens::new(limit + BLOCK_CLOSE_FRAGMENT.len() as u64)
                );
            }
            Ok(admission) => panic!(
                "the closed render is {} tokens over a {limit} limit: {admission:?}",
                admission.ledger().total_with_headroom().get()
            ),
            other => panic!("{other:?}"),
        }
    }
}

/// A heuristic whose count scales with the rendered prefix prices the close
/// higher after a group is admitted than the reserve taken before the scan.
/// The budget is set so the group fits the scan exactly; the closed render
/// then exceeds the limit by the drift, and the phase refuses rather than
/// returning it.
#[test]
fn a_close_priced_above_its_reserve_after_admission_refuses_the_optional_phase() {
    let a = tool_span("opt-a", "1", "aaaa");
    let fixture = Fixture::new(&[REQUIRED, a]);
    let prefix_scaled =
        AccountingProfile::heuristic("prefix-scaled", "len*(1+groups)", 0, |text| {
            text.len() * (1 + text.matches("</group>").count())
        });
    let (probe, _) = run_with_profile(&fixture, &[optional(&a)], &wide(), 1 << 20, &prefix_scaled);
    let probe = probe.unwrap();
    assert_eq!(probe.admitted().len(), 1);
    let closed = probe.ledger().total_with_headroom().get();
    let close_entry = probe
        .ledger()
        .entries()
        .iter()
        .find(|entry| entry.item == Charged::BlockClose)
        .unwrap()
        .charge
        .with_headroom()
        .get();
    let reserve = BLOCK_CLOSE_FRAGMENT.len() as u64;
    assert!(
        close_entry > reserve,
        "the profile must drift: {close_entry} vs {reserve}"
    );
    let limit = closed - (close_entry - reserve);
    let (result, _) = run_with_profile(&fixture, &[optional(&a)], &wide(), limit, &prefix_scaled);
    match result {
        Err(PreparationRefusal::CloseOverBudget {
            limit: reported,
            charged,
        }) => {
            assert_eq!(reported, ClaudeTokens::new(limit));
            assert_eq!(charged, ClaudeTokens::new(closed));
        }
        Ok(admission) => panic!(
            "the closed render is {} tokens over a {limit} limit: {admission:?}",
            admission.ledger().total_with_headroom().get()
        ),
        other => panic!("{other:?}"),
    }
}

/// The close is priced after the last deadline poll of the scan; a budget
/// that ends inside that pricing still refuses the phase.
#[test]
fn a_budget_that_ends_while_the_close_is_priced_refuses_the_optional_phase() {
    let a = tool_span("opt-a", "1", "aaaa");
    let fixture = Fixture::new(&[REQUIRED, a]);
    let budget = EvalBudget::unbounded();
    let cancelling = budget.clone();
    let profile = AccountingProfile::heuristic("cancels-at-close", "cancels", 0, move |text| {
        if text.ends_with(&format!("</group>\n{BLOCK_CLOSE_FRAGMENT}")) {
            cancelling.cancel();
        }
        text.len()
    });
    let mut trace = PackingTrace::default();
    let inputs = RequiredInputs {
        kernel: &fixture.kernel,
        project: &fixture.project,
        destination: kernel::ArtifactDestination::Local,
        budget: &budget,
        profile: &profile,
    };
    let required = prepare_required(
        &fixture.store,
        inputs,
        &[REQUIRED.request()],
        &bounds(1 << 20),
        &accounting_bounds(),
        &mut trace,
    )
    .unwrap();
    let result = prepare_optional(
        &fixture.store,
        inputs,
        &required,
        &[optional(&a)],
        &wide(),
        &accounting_bounds(),
        &mut trace,
    );
    assert_eq!(result.unwrap_err(), PreparationRefusal::Deadline);
}

#[test]
fn optional_faults_are_excluded_with_a_reason_and_never_refuse_the_preparation() {
    let live = tool_span("opt-live", "1", "live");
    let unknown = tool_span("opt-unknown", "1", "never");
    let excluded = tool_span("opt-excluded", "1", "excluded");
    let moved = tool_span("opt-moved", "1", "moved on");
    let tombstoned = tool_span("opt-tombstoned", "1", "retired");
    let fixture = Fixture::with_admitted(
        &[REQUIRED, live, excluded, moved, tombstoned],
        &["req", "opt-live", "opt-moved", "opt-tombstoned"],
    );
    fixture
        .store
        .with_conn_fenced(|conn| {
            tombstone_occurrence(
                conn,
                &tombstoned.id().to_string(),
                Tombstone {
                    invalidated_commit_seq: 9,
                    reason: TombstoneReason::Retired,
                },
                2,
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    let stale = OptionalRequest {
        occurrence: moved.id(),
        revision: 2,
    };
    let requests = [
        optional(&live),
        optional(&unknown),
        optional(&excluded),
        stale,
        optional(&tombstoned),
        optional(&live),
        optional(&unknown),
    ];
    let (result, trace) = run(&fixture, &requests, &wide(), 1 << 20);
    let admission = result.unwrap();
    assert_eq!(admission.admitted().len(), 1);
    // `Duplicate` names the later request; the identity's first request still
    // carries its own reason, so a repeated missing identity reports both.
    assert_eq!(
        admission.excluded(),
        vec![
            (live.id(), OptionalExclusion::Duplicate),
            (unknown.id(), OptionalExclusion::Duplicate),
            (unknown.id(), OptionalExclusion::Missing),
            (
                excluded.id(),
                OptionalExclusion::Excluded(EligibilityVerdict::Retracted)
            ),
            (moved.id(), OptionalExclusion::Stale),
            (tombstoned.id(), OptionalExclusion::Stale),
        ]
    );
    assert_eq!(
        trace.payload_loads(),
        2,
        "the duplicate is read and loaded once"
    );
}

#[test]
fn an_optional_bound_at_limit_plus_one_refuses_with_the_bound_before_any_load() {
    let a = tool_span("opt-a", "1", "aaaa");
    let b = tool_span("opt-b", "1", "bbbb");
    let fixture = Fixture::new(&[REQUIRED, a, b]);
    let requests = [optional(&a), optional(&b)];
    let two = OptionalBounds {
        max_fused_candidates: NonZeroUsize::new(2).unwrap(),
        ..wide()
    };
    let (ok, _) = run(&fixture, &requests, &two, 1 << 20);
    assert_eq!(ok.unwrap().admitted().len(), 2);
    let one = OptionalBounds {
        max_fused_candidates: NonZeroUsize::MIN,
        ..wide()
    };
    let (refused, trace) = run(&fixture, &requests, &one, 1 << 20);
    match refused.unwrap_err() {
        PreparationRefusal::OptionalBound(exceeded) => {
            assert_eq!(exceeded.bound, OptionalBound::FusedCandidates);
            assert_eq!(exceeded.at, 1);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        trace.payload_loads(),
        1,
        "only the required payload was loaded"
    );
}

/// The kernel judges at most `MAX_ELIGIBILITY_CANDIDATES` per batch, so a
/// caller bound above it cannot be honored; the set is refused as a bound
/// violation before any row is read rather than read whole and refused by
/// the kernel.
#[test]
fn a_fused_set_beyond_the_kernel_batch_cap_is_refused_before_any_read() {
    let fixture = Fixture::new(&[REQUIRED]);
    let requests: Vec<OptionalRequest> = (0..=kernel::MAX_ELIGIBILITY_CANDIDATES)
        .map(|index| OptionalRequest {
            occurrence: OccurrenceId::parse(&format!("{index:064x}")).unwrap(),
            revision: 1,
        })
        .collect();
    let generous = OptionalBounds {
        max_fused_candidates: NonZeroUsize::new(2 * kernel::MAX_ELIGIBILITY_CANDIDATES).unwrap(),
        ..wide()
    };
    let (result, trace) = run(&fixture, &requests, &generous, 1 << 20);
    match result.unwrap_err() {
        PreparationRefusal::OptionalBound(exceeded) => {
            assert_eq!(exceeded.bound, OptionalBound::FusedCandidates);
            assert_eq!(exceeded.at, kernel::MAX_ELIGIBILITY_CANDIDATES);
        }
        other => panic!("{other:?}"),
    }
    assert!(
        !trace
            .events()
            .iter()
            .any(|event| matches!(event, StageEvent::Optional(..))),
        "no optional row is read before the cap refuses the set: {:?}",
        trace.events()
    );
}

/// A corrupt optional payload is excluded with its reason after the load; the
/// scan continues and the preparation is not refused.
#[test]
fn a_corrupt_optional_payload_is_excluded_and_the_scan_continues() {
    let damaged = tool_span("opt-a", "1", "damaged");
    let sound = tool_span("opt-b", "1", "sound");
    let fixture = Fixture::new(&[REQUIRED, damaged, sound]);
    let payload_id = kernel::source_identity::payload_id(damaged.payload.as_bytes());
    let mut altered = damaged.payload.as_bytes().to_vec();
    altered[0] ^= 1;
    rusqlite::Connection::open(fixture.sqlite_path())
        .unwrap()
        .execute(
            "UPDATE payloads SET bytes=?2 WHERE payload_id=?1",
            rusqlite::params![payload_id, altered],
        )
        .unwrap();
    let (result, trace) = run(
        &fixture,
        &[optional(&damaged), optional(&sound)],
        &wide(),
        1 << 20,
    );
    let admission = result.unwrap();
    assert_eq!(
        admission.excluded(),
        vec![(damaged.id(), OptionalExclusion::Corrupt)]
    );
    assert_eq!(admission.admitted().len(), 1);
    assert_eq!(
        admission.admitted()[0].group.members().collect::<Vec<_>>(),
        vec![sound.id()]
    );
    assert_eq!(
        trace.payload_loads(),
        2,
        "the required payload and the sound optional payload; the corrupt load is not charged"
    );
}

#[test]
fn a_deadline_that_passes_while_the_connection_is_held_refuses_the_optional_phase_without_reading()
{
    let a = tool_span("opt-a", "1", "aaaa");
    let fixture = Fixture::new(&[REQUIRED, a]);
    let (required, _) = fixture.prepare(
        &[REQUIRED.request()],
        &bounds(1 << 20),
        &EvalBudget::unbounded(),
    );
    let required = required.unwrap();
    let short = EvalBudget::new(
        Some(Instant::now() + Duration::from_millis(200)),
        Arc::new(AtomicBool::new(false)),
    );
    let mut trace = PackingTrace::default();
    let profile = byte_profile();
    let (waited, result) = while_connection_is_held(&fixture, || {
        prepare_optional(
            &fixture.store,
            RequiredInputs {
                kernel: &fixture.kernel,
                project: &fixture.project,
                destination: kernel::ArtifactDestination::Local,
                budget: &short,
                profile: &profile,
            },
            &required,
            &[optional(&a)],
            &wide(),
            &accounting_bounds(),
            &mut trace,
        )
    });
    assert_eq!(result.unwrap_err(), PreparationRefusal::Deadline);
    assert!(
        waited < Duration::from_secs(2),
        "the hold must end at the deadline, not when the holder releases: {waited:?}"
    );
    assert!(
        trace.events().is_empty(),
        "no optional row is read after the deadline: {:?}",
        trace.events()
    );
}

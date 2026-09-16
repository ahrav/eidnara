//! The optional phase runs after the required phase over the budget it left,
//! groups by key, and admits groups by skip-and-continue in fused order.

mod support;

use std::num::{NonZeroU64, NonZeroUsize};

use daemon::packing::{
    Charged, ClaudeTokens, OptionalAdmission, OptionalExclusion, OptionalRequest, PackingTrace,
    PreparationRefusal, RequiredInputs, StageEvent, prepare_optional, prepare_required, render,
};
use kernel::EligibilityVerdict;
use kernel::applicability::EvalBudget;
use retrieval::packing::{OptionalBound, OptionalBounds};
use support::packing::{
    Fixture, ToolSpan, accounting_bounds, bounds, byte_profile, required_render_total, tool_range,
    tool_span,
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
    let mut trace = PackingTrace::default();
    trace.note_retrieval_call();
    let eval = EvalBudget::unbounded();
    let profile = byte_profile();
    let inputs = RequiredInputs {
        kernel: &fixture.kernel,
        project: &fixture.project,
        destination: kernel::ArtifactDestination::Local,
        budget: &eval,
        profile: &profile,
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
    let required_charge = required_render_total(&[REQUIRED]) - required_render_total(&[]);

    let (generous, _) = run(&fixture, &requests, &wide(), 1 << 20);
    let generous = generous.unwrap();
    let cost_of = |first_fused: usize| {
        generous
            .admitted
            .iter()
            .find(|group| group.group.first_fused == first_fused)
            .map(|group| group.cost.get())
            .unwrap()
    };
    let (big, small, medium) = (cost_of(0), cost_of(1), cost_of(2));
    assert!(small < medium && medium < big, "{small} {medium} {big}");
    for group in &generous.admitted {
        assert_eq!(
            group.cost,
            ClaudeTokens::new(render::group_fragment(&group.group).len() as u64),
            "a group's cost is its rendered delta under the byte profile"
        );
    }
    assert_eq!(
        generous
            .ledger
            .entries()
            .iter()
            .map(|entry| entry.bytes)
            .sum::<usize>(),
        generous.ledger.text().len()
    );

    let remaining = small + medium;
    assert!(big > remaining, "the first group alone must not fit");
    let (result, trace) = run(&fixture, &requests, &wide(), required_charge + remaining);
    let admission = result.unwrap();
    let admitted: Vec<_> = admission
        .admitted
        .iter()
        .map(|group| (group.group.first_fused, group.cost.get()))
        .collect();
    assert_eq!(admitted, vec![(1, small), (2, medium)]);
    assert_eq!(admission.skipped.len(), 1);
    assert_eq!(admission.skipped[0].group.first_fused, 0);
    assert_eq!(admission.skipped[0].cost, ClaudeTokens::new(big));
    assert_eq!(admission.remaining, ClaudeTokens::new(0));
    assert!(admission.excluded.is_empty());
    assert!(admission.ungrouped.is_empty());
    assert_eq!(trace.payload_loads(), 4);
    optional_starts_after_the_last_required_event(&trace);
    let text = admission.ledger.text();
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
        required_charge + remaining + 3,
    );
    assert_eq!(
        spare.unwrap().remaining,
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
    assert_eq!(admission.admitted.len(), 2);
    assert_eq!(admission.admitted[0].group.first_fused, 0);
    let grouped = &admission.admitted[1];
    assert_eq!(grouped.group.first_fused, 1);
    assert_eq!(grouped.group.ranges.len(), 2);
    assert_eq!(grouped.group.ranges[0].bytes, PARENT.as_bytes()[0..10]);
    assert_eq!(grouped.group.ranges[1].bytes, PARENT.as_bytes()[15..20]);
    assert_eq!(
        grouped.cost,
        ClaudeTokens::new(render::group_fragment(&grouped.group).len() as u64)
    );
    let wrapper_entries: Vec<_> = admission
        .ledger
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
        .ledger
        .entries()
        .iter()
        .filter(|entry| matches!(entry.item, Charged::Range(1, _)))
        .count();
    assert_eq!(range_entries, 2);
}

#[test]
fn optional_faults_are_excluded_with_a_reason_and_never_refuse_the_preparation() {
    let live = tool_span("opt-live", "1", "live");
    let unknown = tool_span("opt-unknown", "1", "never");
    let excluded = tool_span("opt-excluded", "1", "excluded");
    let moved = tool_span("opt-moved", "1", "moved on");
    let fixture = Fixture::with_admitted(
        &[REQUIRED, live, excluded, moved],
        &["req", "opt-live", "opt-moved"],
    );
    let stale = OptionalRequest {
        occurrence: moved.id(),
        revision: 2,
    };
    let requests = [
        optional(&live),
        optional(&unknown),
        optional(&excluded),
        stale,
        optional(&live),
    ];
    let (result, trace) = run(&fixture, &requests, &wide(), 1 << 20);
    let admission = result.unwrap();
    assert_eq!(admission.admitted.len(), 1);
    assert_eq!(
        admission.excluded,
        vec![
            (live.id(), OptionalExclusion::Duplicate),
            (unknown.id(), OptionalExclusion::Missing),
            (
                excluded.id(),
                OptionalExclusion::Excluded(EligibilityVerdict::Retracted)
            ),
            (moved.id(), OptionalExclusion::Stale),
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
    assert_eq!(ok.unwrap().admitted.len(), 2);
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

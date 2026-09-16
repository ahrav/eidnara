//! The optional phase runs after the required phase over the budget it left,
//! groups by key, and admits groups by skip-and-continue in fused order.

mod support;

use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use daemon::packing::{
    ClaudeTokens, CostEstimator, OptionalAdmission, OptionalExclusion, OptionalRequest,
    PackingTrace, PreparationRefusal, RequiredInputs, StageEvent, prepare_optional,
    prepare_required,
};
use kernel::EligibilityVerdict;
use kernel::applicability::EvalBudget;
use retrieval::fusion::OccurrenceId;
use retrieval::packing::{OptionalBound, OptionalBounds};
use retrieval::{Tombstone, TombstoneReason, tombstone_occurrence};
use support::packing::{
    ByteEstimator, Fixture, ToolSpan, bounds, tool_range, tool_span, while_connection_is_held,
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
    run_with(
        fixture,
        optional_requests,
        optional_bounds,
        budget,
        &ByteEstimator,
    )
}

fn run_with(
    fixture: &Fixture,
    optional_requests: &[OptionalRequest],
    optional_bounds: &OptionalBounds,
    budget: u64,
    estimator: &dyn CostEstimator,
) -> (Result<OptionalAdmission, PreparationRefusal>, PackingTrace) {
    let mut trace = PackingTrace::default();
    trace.note_retrieval_call();
    let eval = EvalBudget::unbounded();
    let inputs = RequiredInputs {
        kernel: &fixture.kernel,
        project: &fixture.project,
        destination: kernel::ArtifactDestination::Local,
        budget: &eval,
        estimator,
    };
    let required = prepare_required(
        &fixture.store,
        inputs,
        &[REQUIRED.request()],
        &bounds(budget),
        &mut trace,
    )
    .unwrap();
    let result = prepare_optional(
        &fixture.store,
        inputs,
        &required,
        optional_requests,
        optional_bounds,
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
    let eleven = tool_span("opt-a", "1", "elevenbytes");
    let four = tool_span("opt-b", "1", "four");
    let six = tool_span("opt-c", "1", "sixsix");
    assert_eq!(
        (eleven.payload.len(), four.payload.len(), six.payload.len()),
        (11, 4, 6)
    );
    let fixture = Fixture::new(&[REQUIRED, eleven, four, six]);
    let budget = REQUIRED.payload.len() as u64 + 10;
    let requests = [optional(&eleven), optional(&four), optional(&six)];
    let (result, trace) = run(&fixture, &requests, &wide(), budget);
    let admission = result.unwrap();
    let admitted: Vec<_> = admission
        .admitted
        .iter()
        .map(|group| (group.group.first_fused, group.cost.get()))
        .collect();
    assert_eq!(admitted, vec![(1, 4), (2, 6)]);
    assert_eq!(admission.skipped.len(), 1);
    assert_eq!(admission.skipped[0].group.first_fused, 0);
    assert_eq!(admission.skipped[0].cost, ClaudeTokens::new(11));
    assert_eq!(admission.remaining, ClaudeTokens::new(0));
    assert!(admission.excluded.is_empty());
    assert!(admission.ungrouped.is_empty());
    assert_eq!(trace.payload_loads(), 4);
    optional_starts_after_the_last_required_event(&trace);

    let (spare, _) = run(&fixture, &requests[1..], &wide(), budget + 3);
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
    let grouped = &admission.admitted[1].group;
    assert_eq!(grouped.first_fused, 1);
    assert_eq!(grouped.ranges.len(), 2);
    assert_eq!(grouped.ranges[0].bytes, PARENT.as_bytes()[0..10]);
    assert_eq!(grouped.ranges[1].bytes, PARENT.as_bytes()[15..20]);
    assert_eq!(admission.admitted[1].cost, ClaudeTokens::new(15));
}

/// Charges nothing for the required payload so the optional phase starts with
/// the whole `u64` range, and more than half of it for every other range.
struct HalfPlusOneEstimator;

impl CostEstimator for HalfPlusOneEstimator {
    fn profile(&self) -> &'static str {
        "half-plus-one"
    }

    fn cost(&self, bytes: &[u8]) -> ClaudeTokens {
        if bytes == REQUIRED.payload.as_bytes() {
            ClaudeTokens::new(0)
        } else {
            ClaudeTokens::new(u64::MAX / 2 + 1)
        }
    }
}

#[test]
fn a_group_whose_cost_overflows_is_refused_not_admitted_at_a_saturated_cost() {
    let a = tool_range("tool", PARENT, 0, 6);
    let c = tool_range("tool", PARENT, 15, 20);
    let fixture = Fixture::new(&[REQUIRED, a, c]);
    let requests = [optional(&a), optional(&c)];
    let (result, _) = run_with(
        &fixture,
        &requests,
        &wide(),
        u64::MAX,
        &HalfPlusOneEstimator,
    );
    match result {
        Err(PreparationRefusal::OptionalCostOverflow { at }) => assert_eq!(at, 0),
        other => panic!("an unrepresentable cost must not be admitted: {other:?}"),
    }
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
        admission.excluded,
        vec![(damaged.id(), OptionalExclusion::Corrupt)]
    );
    assert_eq!(admission.admitted.len(), 1);
    assert_eq!(
        admission.admitted[0].group.members().collect::<Vec<_>>(),
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
    let (waited, result) = while_connection_is_held(&fixture, || {
        prepare_optional(
            &fixture.store,
            RequiredInputs {
                kernel: &fixture.kernel,
                project: &fixture.project,
                destination: kernel::ArtifactDestination::Local,
                budget: &short,
                estimator: &ByteEstimator,
            },
            &required,
            &[optional(&a)],
            &wide(),
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

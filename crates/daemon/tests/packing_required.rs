//! The required phase completes or refuses before any optional event, never
//! retrieves, and never truncates a required payload.

mod support;

use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use daemon::packing::{
    ClaudeTokens, PackingTrace, PreparationRefusal, RequiredEvent, RequiredMaterialization,
    StageEvent,
};
use kernel::EligibilityVerdict;
use kernel::applicability::EvalBudget;
use kernel::source_identity::encode;
use retrieval::packing::{RequiredBound, RequiredBounds, RequiredContextFailure, RequiredRequest};
use retrieval::{Tombstone, TombstoneReason, tombstone_occurrence};
use support::packing::{Fixture, ToolSpan, bounds, required_render_total, tool_span};

fn required_failure(
    result: &Result<RequiredMaterialization, PreparationRefusal>,
) -> &RequiredContextFailure<ClaudeTokens> {
    match result {
        Err(PreparationRefusal::Required(failure)) => failure,
        other => panic!("expected a required-context failure, got {other:?}"),
    }
}

/// The trace enters every call with one lane retrieval already noted, so an
/// entry that retrieved would show two and one that reset the counter zero.
fn assert_no_optional_work(trace: &PackingTrace) {
    assert_eq!(trace.optional_events(), 0);
    assert_eq!(trace.retrieval_calls(), 1);
}

fn required_events(trace: &PackingTrace) -> Vec<RequiredEvent> {
    trace
        .events()
        .iter()
        .map(|event| match event {
            StageEvent::Required(event, _) => *event,
            StageEvent::Optional(..) => panic!("optional event in the required phase"),
        })
        .collect()
}

type Case = (
    &'static str,
    Vec<RequiredRequest>,
    RequiredBounds<ClaudeTokens>,
    RequiredContextFailure<ClaudeTokens>,
);

const FIRST: ToolSpan = tool_span("call-1", "1", "the first required payload\n");
const SECOND: ToolSpan = tool_span("call-2", "1", "the second one, longer by a bit\n");

#[test]
fn required_cost_at_the_limit_succeeds_and_one_above_fails_without_truncation() {
    let fixture = Fixture::new(&[FIRST, SECOND]);
    let requests = [FIRST.request(), SECOND.request()];
    let total = required_render_total(&[FIRST, SECOND]) - required_render_total(&[]);

    let (ok, trace) = fixture.prepare(&requests, &bounds(total), &EvalBudget::unbounded());
    let materialized = ok.unwrap();
    assert_eq!(materialized.charged(), ClaudeTokens::new(total));
    assert_eq!(
        materialized.ledger().profile().identity(),
        "one-token-per-byte"
    );
    assert_eq!(
        materialized.ledger().total(),
        ClaudeTokens::new(required_render_total(&[FIRST, SECOND])),
        "the ledger also carries the block open"
    );
    assert_eq!(
        materialized
            .ledger()
            .entries()
            .iter()
            .map(|entry| entry.bytes)
            .sum::<usize>(),
        materialized.ledger().text().len()
    );
    let bytes: Vec<&[u8]> = materialized
        .items()
        .iter()
        .map(|item| item.bytes.as_slice())
        .collect();
    assert_eq!(bytes, vec![FIRST.selected_bytes(), SECOND.selected_bytes()]);
    assert_eq!(
        materialized
            .items()
            .iter()
            .map(|item| item.cost.get())
            .sum::<u64>(),
        total,
        "every rendered required byte is charged"
    );
    assert_eq!(trace.payload_loads(), 2);
    assert_eq!(
        required_events(&trace),
        [
            RequiredEvent::Read,
            RequiredEvent::Read,
            RequiredEvent::Judged,
            RequiredEvent::Admitted,
            RequiredEvent::Loaded,
            RequiredEvent::Loaded,
            RequiredEvent::Reserved,
        ]
    );
    assert_no_optional_work(&trace);

    let (above, trace) = fixture.prepare(&requests, &bounds(total - 1), &EvalBudget::unbounded());
    assert_eq!(
        required_failure(&above),
        &RequiredContextFailure::OverBudget {
            limit: ClaudeTokens::new(total - 1),
            charged: ClaudeTokens::new(total),
        }
    );
    assert_eq!(trace.payload_loads(), 2, "bytes were loaded, then refused");
    assert_no_optional_work(&trace);

    let (below, _) = fixture.prepare(&requests, &bounds(total + 1), &EvalBudget::unbounded());
    assert_eq!(below.unwrap().charged(), ClaudeTokens::new(total));
}

#[test]
fn each_required_fault_yields_exactly_one_class_with_zero_optional_events() {
    let tombstoned = tool_span("call-3", "1", "retired bytes\n");
    let big = tool_span("call-4", "1", "0123456789abcdef0123456789abcdef");
    let fixture = Fixture::new(&[FIRST, SECOND, tombstoned, big]);
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
    let missing = tool_span("call-9", "1", "never persisted\n");
    let stale = RequiredRequest {
        occurrence: FIRST.id(),
        revision: 2,
    };
    let small_items = RequiredBounds {
        max_item_bytes: NonZeroU64::new(16).unwrap(),
        ..bounds(1 << 20)
    };
    let one_load = RequiredBounds {
        max_payload_loads: NonZeroUsize::MIN,
        ..bounds(1 << 20)
    };
    let few_bytes = RequiredBounds {
        max_payload_bytes: NonZeroU64::new(FIRST.payload.len() as u64 + 3).unwrap(),
        ..bounds(1 << 20)
    };

    let cases: Vec<Case> = vec![
        (
            "missing",
            vec![FIRST.request(), missing.request()],
            bounds(1 << 20),
            RequiredContextFailure::Missing(missing.id()),
        ),
        (
            "stale",
            vec![stale],
            bounds(1 << 20),
            RequiredContextFailure::Stale(FIRST.id()),
        ),
        (
            "stale",
            vec![tombstoned.request()],
            bounds(1 << 20),
            RequiredContextFailure::Stale(tombstoned.id()),
        ),
        (
            "oversized",
            vec![big.request()],
            small_items,
            RequiredContextFailure::Oversized {
                occurrence: big.id(),
                bound: RequiredBound::ItemBytes,
            },
        ),
        (
            "oversized",
            vec![FIRST.request(), SECOND.request()],
            one_load,
            RequiredContextFailure::Oversized {
                occurrence: SECOND.id(),
                bound: RequiredBound::PayloadLoads,
            },
        ),
        (
            "oversized",
            vec![FIRST.request(), SECOND.request()],
            few_bytes,
            RequiredContextFailure::Oversized {
                occurrence: SECOND.id(),
                bound: RequiredBound::PayloadBytes,
            },
        ),
    ];
    for (class, requests, bounds, expected) in cases {
        let (result, trace) = fixture.prepare(&requests, &bounds, &EvalBudget::unbounded());
        let failure = required_failure(&result);
        assert_eq!(failure, &expected);
        assert_eq!(failure.class(), class);
        assert_eq!(
            trace.payload_loads(),
            0,
            "{class}: refused before any byte is loaded"
        );
        assert!(!required_events(&trace).contains(&RequiredEvent::Loaded));
        assert_no_optional_work(&trace);
    }
}

#[test]
fn a_required_occurrence_the_kernel_excludes_is_ineligible_not_missing() {
    let fixture = Fixture::with_admitted(&[FIRST], &[]);
    let (result, trace) = fixture.prepare(
        &[FIRST.request()],
        &bounds(1 << 20),
        &EvalBudget::unbounded(),
    );
    match required_failure(&result) {
        RequiredContextFailure::Ineligible {
            occurrence,
            verdict,
        } => {
            assert_eq!(*occurrence, FIRST.id());
            assert_eq!(*verdict, EligibilityVerdict::Retracted);
        }
        other => panic!("{other:?}"),
    }
    assert_no_optional_work(&trace);
}

#[test]
fn corrupt_payload_bytes_and_foreign_tuples_are_refused_as_corrupt() {
    let fixture = Fixture::new(&[FIRST, SECOND]);
    let path = fixture.sqlite_path();
    let damage = |sql: &str, params: &[&dyn rusqlite::ToSql]| {
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute(sql, params)
            .unwrap();
    };
    let payload_id = kernel::source_identity::payload_id(FIRST.payload.as_bytes());
    let mut altered = FIRST.payload.as_bytes().to_vec();
    altered[0] ^= 1;
    damage(
        "UPDATE payloads SET bytes=?2 WHERE payload_id=?1",
        &[&payload_id, &altered],
    );
    let (result, trace) = fixture.prepare(
        &[FIRST.request()],
        &bounds(1 << 20),
        &EvalBudget::unbounded(),
    );
    assert_eq!(
        required_failure(&result),
        &RequiredContextFailure::Corrupt(FIRST.id())
    );
    assert_eq!(trace.payload_loads(), 0, "the load is refused, not charged");
    assert_eq!(
        required_events(&trace),
        [
            RequiredEvent::Read,
            RequiredEvent::Judged,
            RequiredEvent::Admitted
        ]
    );
    assert_no_optional_work(&trace);
    damage(
        "UPDATE payloads SET bytes=?2 WHERE payload_id=?1",
        &[&payload_id, &FIRST.payload.as_bytes()],
    );

    let identity = SECOND.identity();
    let foreign = encode(&SECOND.occurrence(&identity), SECOND.payload)
        .unwrap()
        .tuple;
    damage(
        "UPDATE occurrences SET tuple=?2 WHERE occurrence_id=?1",
        &[&FIRST.id().to_string(), &foreign],
    );
    let (result, trace) = fixture.prepare(
        &[FIRST.request()],
        &bounds(1 << 20),
        &EvalBudget::unbounded(),
    );
    assert_eq!(
        required_failure(&result),
        &RequiredContextFailure::Corrupt(FIRST.id())
    );
    assert_no_optional_work(&trace);
}

#[test]
fn an_expired_deadline_refuses_the_required_phase_before_any_optional_event() {
    let fixture = Fixture::new(&[FIRST]);
    let expired = EvalBudget::new(
        Some(Instant::now() - Duration::from_secs(1)),
        Arc::new(AtomicBool::new(false)),
    );
    let (result, trace) = fixture.prepare(&[FIRST.request()], &bounds(1 << 20), &expired);
    assert_eq!(result.unwrap_err(), PreparationRefusal::Deadline);
    assert!(trace.events().is_empty());
    assert_no_optional_work(&trace);

    let cancelled = EvalBudget::unbounded();
    cancelled.cancel();
    let (result, _) = fixture.prepare(&[FIRST.request()], &bounds(1 << 20), &cancelled);
    assert_eq!(result.unwrap_err(), PreparationRefusal::Deadline);
}

#[test]
fn a_required_payload_beyond_the_legacy_cut_is_materialized_and_charged_whole() {
    const LEN: usize = 64 * 1024 + 7;
    static BIG_PAYLOAD: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| "x".repeat(LEN));
    let big = tool_span("call-big", "1", BIG_PAYLOAD.as_str());
    let fixture = Fixture::new(&[big]);
    let total = required_render_total(&[big]) - required_render_total(&[]);
    let (result, _) = fixture.prepare(&[big.request()], &bounds(total), &EvalBudget::unbounded());
    let materialized = result.unwrap();
    assert_eq!(materialized.items().len(), 1);
    assert_eq!(materialized.items()[0].bytes.len(), LEN);
    assert_eq!(materialized.items()[0].bytes, BIG_PAYLOAD.as_bytes());
    assert_eq!(materialized.charged(), ClaudeTokens::new(total));
    assert!(
        materialized.charged().get() > LEN as u64,
        "the wrapper is charged too"
    );
}

#[test]
fn more_requests_than_the_load_bound_are_refused_before_any_read() {
    let fixture = Fixture::new(&[FIRST, SECOND]);
    let one_load = RequiredBounds {
        max_payload_loads: NonZeroUsize::MIN,
        ..bounds(1 << 20)
    };
    let (result, trace) = fixture.prepare(
        &[FIRST.request(), SECOND.request()],
        &one_load,
        &EvalBudget::unbounded(),
    );
    assert_eq!(
        required_failure(&result),
        &RequiredContextFailure::Oversized {
            occurrence: SECOND.id(),
            bound: RequiredBound::PayloadLoads,
        }
    );
    assert!(trace.events().is_empty());
    assert_no_optional_work(&trace);
}

#[test]
fn every_failure_class_has_a_distinct_literal() {
    let id = FIRST.id();
    let literals = [
        RequiredContextFailure::<ClaudeTokens>::Missing(id).class(),
        RequiredContextFailure::<ClaudeTokens>::Stale(id).class(),
        RequiredContextFailure::<ClaudeTokens>::Hidden(id).class(),
        RequiredContextFailure::<ClaudeTokens>::Corrupt(id).class(),
        RequiredContextFailure::<ClaudeTokens>::Ineligible {
            occurrence: id,
            verdict: EligibilityVerdict::WrongScope,
        }
        .class(),
        RequiredContextFailure::<ClaudeTokens>::Oversized {
            occurrence: id,
            bound: RequiredBound::ItemBytes,
        }
        .class(),
        RequiredContextFailure::OverBudget {
            limit: ClaudeTokens::new(1),
            charged: ClaudeTokens::new(2),
        }
        .class(),
    ];
    let distinct: std::collections::BTreeSet<_> = literals.iter().collect();
    assert_eq!(distinct.len(), literals.len());
}

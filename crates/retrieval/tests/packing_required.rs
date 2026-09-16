//! Pure required-phase decisions over caller-supplied facts.

use std::num::{NonZeroU64, NonZeroUsize};

use kernel::source_identity::OccurrenceClass;
use kernel::{EligibilityVerdict, Sensitivity};
use retrieval::eligibility::Disposition;
use retrieval::fusion::OccurrenceId;
use retrieval::packing::{
    Grouping, PayloadRef, Provenance, RequiredBound, RequiredBounds, RequiredContextFailure,
    RequiredFact, RequiredRequest, SelectedOccurrence, admit_required, reserve_required,
};
use retrieval::{Tombstone, TombstoneReason};

fn id(byte: u8) -> OccurrenceId {
    OccurrenceId::parse(&format!("{byte:02x}").repeat(32)).unwrap()
}

fn row(byte: u8, revision: i64, byte_length: u64) -> SelectedOccurrence {
    SelectedOccurrence {
        occurrence: id(byte),
        class: OccurrenceClass::CanonicalClaims,
        revision,
        representation: "decision_summary".to_owned(),
        span: None,
        payload: PayloadRef {
            payload_id: format!("{byte:02x}").repeat(32),
            byte_length,
        },
        sensitivity: Sensitivity::Normal,
        provenance: Provenance {
            domain_id: "d".to_owned(),
            source_object_id: format!("object-{byte}"),
            source_evidence_id: "e".to_owned(),
            source_artifact_digest: "0".repeat(64),
            created_commit_seq: 1,
        },
        tombstone: None,
        grouping: Grouping::NonGrouping(OccurrenceClass::CanonicalClaims),
    }
}

fn request(byte: u8, revision: i64) -> RequiredRequest {
    RequiredRequest {
        occurrence: id(byte),
        revision,
    }
}

fn fact<'a>(
    request: RequiredRequest,
    row: &'a SelectedOccurrence,
    disposition: Disposition,
) -> RequiredFact<'a> {
    RequiredFact {
        request,
        row,
        disposition,
    }
}

fn bounds(token_limit: u64) -> RequiredBounds<u64> {
    RequiredBounds {
        max_payload_loads: NonZeroUsize::new(4).unwrap(),
        max_payload_bytes: NonZeroU64::new(100).unwrap(),
        max_item_bytes: NonZeroU64::new(60).unwrap(),
        token_limit,
    }
}

#[test]
fn every_verdict_maps_to_exactly_one_failure_class() {
    let live = row(1, 1, 10);
    let excluded = Disposition::PolicyExcluded;
    let ineligible = |verdict| {
        Err(RequiredContextFailure::Ineligible {
            occurrence: id(1),
            verdict,
        })
    };
    let cases = [
        (Disposition::Eligible, Ok(())),
        (
            excluded(EligibilityVerdict::Hidden),
            Err(RequiredContextFailure::Hidden(id(1))),
        ),
        (
            excluded(EligibilityVerdict::Stale),
            Err(RequiredContextFailure::Stale(id(1))),
        ),
        (
            excluded(EligibilityVerdict::Superseded),
            Err(RequiredContextFailure::Stale(id(1))),
        ),
        (
            excluded(EligibilityVerdict::Retracted),
            ineligible(EligibilityVerdict::Retracted),
        ),
        (
            excluded(EligibilityVerdict::WrongScope),
            ineligible(EligibilityVerdict::WrongScope),
        ),
        (
            excluded(EligibilityVerdict::ProviderSensitive),
            ineligible(EligibilityVerdict::ProviderSensitive),
        ),
    ];
    for (disposition, expected) in cases {
        let facts = [fact(request(1, 1), &live, disposition)];
        let outcome = admit_required(&facts, &bounds(100)).map(|admitted| {
            assert_eq!(admitted.len(), 1);
        });
        assert_eq!(outcome, expected, "{disposition:?}");
    }
}

#[test]
fn stale_rows_and_bounds_are_checked_in_request_order() {
    let mut retired = row(2, 1, 10);
    retired.tombstone = Some(Tombstone {
        invalidated_commit_seq: 5,
        reason: TombstoneReason::Retired,
    });
    let other_revision = row(3, 2, 10);
    let eligible = Disposition::Eligible;
    assert_eq!(
        admit_required(&[fact(request(2, 1), &retired, eligible)], &bounds(100)),
        Err(RequiredContextFailure::Stale(id(2)))
    );
    assert_eq!(
        admit_required(
            &[fact(request(3, 1), &other_revision, eligible)],
            &bounds(100)
        ),
        Err(RequiredContextFailure::Stale(id(3)))
    );

    let big = row(4, 1, 61);
    assert_eq!(
        admit_required(&[fact(request(4, 1), &big, eligible)], &bounds(100)),
        Err(RequiredContextFailure::Oversized {
            occurrence: id(4),
            bound: RequiredBound::ItemBytes,
        })
    );
    let rows: Vec<_> = (10..15).map(|byte| row(byte, 1, 30)).collect();
    let facts: Vec<_> = rows
        .iter()
        .map(|row| fact(request(row.occurrence.as_bytes()[0], 1), row, eligible))
        .collect();
    assert_eq!(
        admit_required(&facts[..3], &bounds(100)).map(|admitted| admitted.len()),
        Ok(3)
    );
    assert_eq!(
        admit_required(&facts[..4], &bounds(100)),
        Err(RequiredContextFailure::Oversized {
            occurrence: id(13),
            bound: RequiredBound::PayloadBytes,
        })
    );
    let wide = RequiredBounds {
        max_payload_bytes: NonZeroU64::new(1_000).unwrap(),
        ..bounds(100)
    };
    assert_eq!(
        admit_required(&facts, &wide),
        Err(RequiredContextFailure::Oversized {
            occurrence: id(14),
            bound: RequiredBound::PayloadLoads,
        })
    );

    let mut hidden_retired_big = row(6, 1, 61);
    hidden_retired_big.tombstone = retired.tombstone;
    let hidden = Disposition::PolicyExcluded(EligibilityVerdict::Hidden);
    assert_eq!(
        admit_required(
            &[fact(request(6, 1), &hidden_retired_big, hidden)],
            &bounds(100)
        ),
        Err(RequiredContextFailure::Stale(id(6)))
    );
    hidden_retired_big.tombstone = None;
    assert_eq!(
        admit_required(
            &[fact(request(6, 1), &hidden_retired_big, hidden)],
            &bounds(100)
        ),
        Err(RequiredContextFailure::Hidden(id(6)))
    );
    assert_eq!(
        admit_required(
            &[fact(request(6, 1), &hidden_retired_big, eligible)],
            &bounds(100)
        ),
        Err(RequiredContextFailure::Oversized {
            occurrence: id(6),
            bound: RequiredBound::ItemBytes,
        })
    );
    let live = row(7, 1, 10);
    assert_eq!(
        admit_required(&[fact(request(8, 1), &live, eligible)], &bounds(100)),
        Err(RequiredContextFailure::Corrupt(id(8))),
        "a row that is not the requested occurrence is refused"
    );

    let big_then_stale = [
        fact(request(4, 1), &big, eligible),
        fact(request(2, 1), &retired, eligible),
    ];
    assert_eq!(
        admit_required(&big_then_stale, &bounds(100)),
        Err(RequiredContextFailure::Oversized {
            occurrence: id(4),
            bound: RequiredBound::ItemBytes,
        })
    );
}

/// The load-count bound is checked over the whole set before per-request faults.
#[test]
fn the_load_bound_is_checked_over_the_whole_set_before_any_request_fault() {
    let mut retired = row(2, 1, 10);
    retired.tombstone = Some(Tombstone {
        invalidated_commit_seq: 5,
        reason: TombstoneReason::Retired,
    });
    let live = row(3, 1, 10);
    let one_load = RequiredBounds {
        max_payload_loads: NonZeroUsize::MIN,
        ..bounds(100)
    };
    let stale_then_live = [
        fact(request(2, 1), &retired, Disposition::Eligible),
        fact(request(3, 1), &live, Disposition::Eligible),
    ];
    assert_eq!(
        admit_required(&stale_then_live, &one_load),
        Err(RequiredContextFailure::Oversized {
            occurrence: id(3),
            bound: RequiredBound::PayloadLoads,
        })
    );
    assert_eq!(
        admit_required(&stale_then_live[..1], &one_load),
        Err(RequiredContextFailure::Stale(id(2)))
    );
}

#[test]
fn reservation_charges_every_byte_and_stops_exactly_at_the_limit() {
    let rows = [row(1, 1, 4), row(2, 1, 6)];
    let facts: Vec<_> = rows
        .iter()
        .map(|row| {
            fact(
                request(row.occurrence.as_bytes()[0], 1),
                row,
                Disposition::Eligible,
            )
        })
        .collect();
    let admitted = admit_required(&facts, &bounds(100)).unwrap();
    let bytes: [&[u8]; 2] = [b"abcd", b"efghij"];
    let per_byte = |_: &retrieval::packing::AdmittedRequired<'_>, bytes: &[u8]| bytes.len() as u64;

    let reserved = reserve_required(&admitted, &bytes, 10, per_byte).unwrap();
    assert_eq!(reserved.charged, 10);
    assert_eq!(reserved.costs, vec![4, 6]);
    assert_eq!(reserved.costs.iter().sum::<u64>(), reserved.charged);
    assert_eq!(
        reserve_required(&admitted, &bytes, 9, per_byte),
        Err(RequiredContextFailure::OverBudget {
            limit: 9,
            charged: 10,
        })
    );
    assert_eq!(
        reserve_required(&admitted, &bytes, 11, per_byte).map(|r| r.charged),
        Ok(10)
    );

    let short: [&[u8]; 2] = [b"abc", b"efghij"];
    assert_eq!(
        reserve_required(&admitted, &short, 100, per_byte),
        Err(RequiredContextFailure::Corrupt(id(1)))
    );
    assert_eq!(
        reserve_required(&admitted, &bytes[..1], 100, per_byte),
        Err(RequiredContextFailure::Corrupt(id(2))),
        "an admitted item without bytes is never charged as zero"
    );
    let three = [row(1, 1, 4), row(2, 1, 6), row(3, 1, 5)];
    let three_facts: Vec<_> = three
        .iter()
        .map(|row| {
            fact(
                request(row.occurrence.as_bytes()[0], 1),
                row,
                Disposition::Eligible,
            )
        })
        .collect();
    let three_admitted = admit_required(&three_facts, &bounds(100)).unwrap();
    let three_bytes: [&[u8]; 3] = [b"abcd", b"efghij", b"klmno"];
    assert_eq!(
        reserve_required(&three_admitted, &three_bytes, 12, per_byte),
        Err(RequiredContextFailure::OverBudget {
            limit: 12,
            charged: 15,
        }),
        "charged is the sum through the item that crossed the limit"
    );
    assert_eq!(
        reserve_required(&admitted, &bytes, u64::MAX, |_, _| u64::MAX),
        Err(RequiredContextFailure::OverBudget {
            limit: u64::MAX,
            charged: u64::MAX,
        })
    );
}

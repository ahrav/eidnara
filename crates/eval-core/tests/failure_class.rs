use std::collections::BTreeSet;

use eval_core::{
    Cell, ChainStage, Delivery, DurableState, FAILURE_CLASS_TABLE_DIGEST,
    FAILURE_CLASS_TABLE_PROTOCOL, FailureClass, Outcome, Slice, StageVerdict, cells, classify,
    serialize_table, table_digest,
};

/// The table as a reviewer reads it: one row per (durable state, delivery),
/// with the live and cassette classes for a failing task. A pass is never
/// classified, whatever the other axes say.
const FAILING_ROWS: [(DurableState, Delivery, FailureClass, FailureClass); 12] = [
    (
        DurableState::Held,
        Delivery::Clean,
        FailureClass::Reasoning,
        FailureClass::Indeterminate,
    ),
    (
        DurableState::Held,
        Delivery::FirstLoss,
        FailureClass::Interference,
        FailureClass::Interference,
    ),
    (
        DurableState::Held,
        Delivery::StaleIngress,
        FailureClass::Interference,
        FailureClass::Interference,
    ),
    (
        DurableState::Held,
        Delivery::Indeterminate,
        FailureClass::Indeterminate,
        FailureClass::Indeterminate,
    ),
    (
        DurableState::Refused,
        Delivery::Clean,
        FailureClass::Indeterminate,
        FailureClass::Indeterminate,
    ),
    (
        DurableState::Refused,
        Delivery::FirstLoss,
        FailureClass::DurableState,
        FailureClass::DurableState,
    ),
    (
        DurableState::Refused,
        Delivery::StaleIngress,
        FailureClass::DurableState,
        FailureClass::DurableState,
    ),
    (
        DurableState::Refused,
        Delivery::Indeterminate,
        FailureClass::DurableState,
        FailureClass::DurableState,
    ),
    (
        DurableState::Unknown,
        Delivery::Clean,
        FailureClass::Indeterminate,
        FailureClass::Indeterminate,
    ),
    (
        DurableState::Unknown,
        Delivery::FirstLoss,
        FailureClass::Indeterminate,
        FailureClass::Indeterminate,
    ),
    (
        DurableState::Unknown,
        Delivery::StaleIngress,
        FailureClass::Indeterminate,
        FailureClass::Indeterminate,
    ),
    (
        DurableState::Unknown,
        Delivery::Indeterminate,
        FailureClass::Indeterminate,
        FailureClass::Indeterminate,
    ),
];

#[test]
fn the_table_has_forty_eight_distinct_cells_in_axis_order() {
    let all = cells();
    assert_eq!(all.len(), 48);
    let distinct: BTreeSet<Cell> = all.iter().copied().collect();
    assert_eq!(distinct.len(), 48);
    assert_eq!(
        all[0],
        Cell {
            delivery: Delivery::Clean,
            durable_state: DurableState::Held,
            slice: Slice::Live,
            outcome: Outcome::Pass,
        }
    );
    assert_eq!(
        all[47],
        Cell {
            delivery: Delivery::Indeterminate,
            durable_state: DurableState::Unknown,
            slice: Slice::Cassette,
            outcome: Outcome::Fail,
        }
    );
}

#[test]
fn every_cell_agrees_with_the_hand_authored_rows() {
    let mut reached = BTreeSet::new();
    for cell in cells() {
        let class = classify(cell);
        let expected = match cell.outcome {
            Outcome::Pass => None,
            Outcome::Fail => {
                let (_, _, live, cassette) = FAILING_ROWS
                    .iter()
                    .find(|(state, delivery, _, _)| {
                        *state == cell.durable_state && *delivery == cell.delivery
                    })
                    .unwrap();
                Some(match cell.slice {
                    Slice::Live => *live,
                    Slice::Cassette => *cassette,
                })
            }
        };
        assert_eq!(class, expected, "{cell:?}");
        reached.insert(cell);
    }
    assert_eq!(reached.len(), 48, "every cell is reached");
    let classes: BTreeSet<Option<FailureClass>> = cells().into_iter().map(classify).collect();
    assert_eq!(
        classes,
        BTreeSet::from([
            None,
            Some(FailureClass::Interference),
            Some(FailureClass::Reasoning),
            Some(FailureClass::DurableState),
            Some(FailureClass::Indeterminate),
        ])
    );
}

#[test]
fn reasoning_is_claimable_only_in_the_live_slice() {
    for cell in cells() {
        if classify(cell) == Some(FailureClass::Reasoning) {
            assert_eq!(cell.slice, Slice::Live, "{cell:?}");
            assert_eq!(cell.delivery, Delivery::Clean);
            assert_eq!(cell.durable_state, DurableState::Held);
        }
        if cell.slice == Slice::Cassette {
            assert_ne!(classify(cell), Some(FailureClass::Reasoning), "{cell:?}");
        }
    }
}

#[test]
fn the_serialized_table_digests_to_the_pinned_constant() {
    assert_eq!(
        FAILURE_CLASS_TABLE_PROTOCOL,
        "eidnara-failure-class-table-v1"
    );
    assert_eq!(table_digest(), FAILURE_CLASS_TABLE_DIGEST);
    let table = serialize_table();
    let rows = table.as_array().unwrap();
    assert_eq!(rows.len(), 48);
    assert_eq!(
        rows[0],
        serde_json::json!({
            "cell": {
                "delivery": "clean",
                "durable_state": "held",
                "slice": "live",
                "outcome": "pass"
            },
            "class": null
        })
    );
    assert_eq!(rows[1]["class"], "reasoning");
    assert_eq!(rows[3]["class"], "indeterminate");
}

#[test]
fn a_ledger_verdict_maps_to_its_delivery_without_the_stage() {
    assert_eq!(
        Delivery::of(StageVerdict::FirstLoss(ChainStage::Packing)),
        Delivery::FirstLoss
    );
    assert_eq!(
        Delivery::of(StageVerdict::FirstLoss(ChainStage::Exact)),
        Delivery::FirstLoss
    );
    assert_eq!(
        Delivery::of(StageVerdict::StaleIngress(ChainStage::Fusion)),
        Delivery::StaleIngress
    );
    assert_eq!(
        Delivery::of(StageVerdict::<ChainStage>::Clean),
        Delivery::Clean
    );
    assert_eq!(
        Delivery::of(StageVerdict::<ChainStage>::Indeterminate),
        Delivery::Indeterminate
    );
}

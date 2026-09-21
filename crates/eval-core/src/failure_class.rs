use context_core::canonical_json::protocol_digest;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ledger::{Stage, StageVerdict};

pub const FAILURE_CLASS_TABLE_PROTOCOL: &str = "eidnara-failure-class-table-v1";
/// The `FAILURE_CLASS_TABLE_PROTOCOL` digest of [`serialize_table`]; the
/// manifest carries it so a report names the table it was classified under.
pub const FAILURE_CLASS_TABLE_DIGEST: &str =
    "03a1eabc09e28bb01ae1db0e460c8580297da1a9e249faf7153c1cdda6807fed";

/// What the stage ledger said about the required evidence: the four verdicts
/// with the stage dropped, because the class does not depend on which stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Clean,
    FirstLoss,
    StaleIngress,
    Indeterminate,
}

impl Delivery {
    pub const ALL: [Self; 4] = [
        Self::Clean,
        Self::FirstLoss,
        Self::StaleIngress,
        Self::Indeterminate,
    ];

    pub fn of<S: Stage>(verdict: StageVerdict<S>) -> Self {
        match verdict {
            StageVerdict::Clean => Self::Clean,
            StageVerdict::FirstLoss(_) => Self::FirstLoss,
            StageVerdict::StaleIngress(_) => Self::StaleIngress,
            StageVerdict::Indeterminate => Self::Indeterminate,
        }
    }
}

/// Whether the store held the required knowledge when the task ran, as the
/// kernel's own verdict says: held, refused by a typed retention verdict, or
/// unknown because no read-back settled it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableState {
    Held,
    Refused,
    Unknown,
}

impl DurableState {
    pub const ALL: [Self; 3] = [Self::Held, Self::Refused, Self::Unknown];
}

/// Model traffic was live or replayed from a cassette; only a live model can
/// be blamed for its reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slice {
    Live,
    Cassette,
}

impl Slice {
    pub const ALL: [Self; 2] = [Self::Live, Self::Cassette];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    Fail,
}

impl Outcome {
    pub const ALL: [Self; 2] = [Self::Pass, Self::Fail];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    pub delivery: Delivery,
    pub durable_state: DurableState,
    pub slice: Slice,
    pub outcome: Outcome,
}

/// Every cell in table order: delivery, then durable state, slice, outcome.
pub fn cells() -> Vec<Cell> {
    let mut out = Vec::with_capacity(48);
    for delivery in Delivery::ALL {
        for durable_state in DurableState::ALL {
            for slice in Slice::ALL {
                for outcome in Outcome::ALL {
                    out.push(Cell {
                        delivery,
                        durable_state,
                        slice,
                        outcome,
                    });
                }
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// The store held the knowledge and the chain lost it or served stale
    /// evidence in its place.
    Interference,
    /// The knowledge was held and delivered, and a live model still failed.
    Reasoning,
    /// The store itself refused or lost the knowledge.
    DurableState,
    Indeterminate,
}

/// A passing task has no failure to classify; a failing one takes its class
/// from delivered evidence and durable state, never from model narrative.
pub fn classify(cell: Cell) -> Option<FailureClass> {
    if cell.outcome == Outcome::Pass {
        return None;
    }
    Some(match (cell.durable_state, cell.delivery, cell.slice) {
        (DurableState::Unknown, _, _) => FailureClass::Indeterminate,
        (DurableState::Refused, Delivery::Clean, _) => FailureClass::Indeterminate,
        (DurableState::Refused, _, _) => FailureClass::DurableState,
        (DurableState::Held, Delivery::FirstLoss | Delivery::StaleIngress, _) => {
            FailureClass::Interference
        }
        (DurableState::Held, Delivery::Clean, Slice::Live) => FailureClass::Reasoning,
        (DurableState::Held, Delivery::Clean, Slice::Cassette) => FailureClass::Indeterminate,
        (DurableState::Held, Delivery::Indeterminate, _) => FailureClass::Indeterminate,
    })
}

/// The whole table as one canonical value: an array of `{cell, class}` rows in
/// [`cells`] order, with `class` `null` for a pass.
pub fn serialize_table() -> Value {
    Value::Array(
        cells()
            .into_iter()
            .map(|cell| {
                serde_json::json!({
                    "cell": cell,
                    "class": classify(cell),
                })
            })
            .collect(),
    )
}

pub fn table_digest() -> String {
    protocol_digest(FAILURE_CLASS_TABLE_PROTOCOL, &serialize_table())
        .expect("the table is canonical JSON")
}

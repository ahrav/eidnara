use serde::{Deserialize, Serialize};

use crate::ledger::{Stage, StageKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface1Stage {
    TailEligibility,
    Suppression,
    LengthGate,
    TokenGate,
    CandidateWindow,
    MatchFilter,
    Threshold,
    Cap,
    Render,
    DecisionFreeze,
    Deferral,
    OverlayApply,
    Attachment,
}

/// Surface 1 stages in production order; the index is the ledger ordinal.
pub const SURFACE1_STAGES: [Surface1Stage; 13] = [
    Surface1Stage::TailEligibility,
    Surface1Stage::Suppression,
    Surface1Stage::LengthGate,
    Surface1Stage::TokenGate,
    Surface1Stage::CandidateWindow,
    Surface1Stage::MatchFilter,
    Surface1Stage::Threshold,
    Surface1Stage::Cap,
    Surface1Stage::Render,
    Surface1Stage::DecisionFreeze,
    Surface1Stage::Deferral,
    Surface1Stage::OverlayApply,
    Surface1Stage::Attachment,
];

/// Every surface-1 stage is a gate or a filter over the session's segments;
/// the candidates enter at the tail, so a required occurrence's path is the
/// whole list.
impl Stage for Surface1Stage {
    const ALL: &'static [Self] = &SURFACE1_STAGES;
    const REACHABILITY: Reachability = Reachability::DefaultProduction;

    fn kind(self) -> StageKind {
        StageKind::Filter
    }
}

/// The surfaces a paired campaign evaluates. Surfaces 1 and 3 are
/// default-production; surface 2, the query route, and packing are activated
/// components. Surface 1's recency bound is `SURFACE1_HINT_BOUNDS.candidates`;
/// the others have no production constant and must declare theirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluatedSurface {
    Surface1,
    Surface2,
    Surface3,
    QueryRoute,
    Packing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HintBounds {
    pub candidates: usize,
    pub query_tokens: usize,
    pub results: usize,
    pub matched_tokens: usize,
    pub fragment_units: usize,
    pub total_units: usize,
}

pub const SURFACE1_HINT_BOUNDS: HintBounds = HintBounds {
    candidates: 100,
    query_tokens: 24,
    results: 3,
    matched_tokens: 2,
    fragment_units: 80,
    total_units: 800,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reachability {
    DefaultProduction,
    ExplicitConfigOnly,
    TestOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Construction {
    Replay,
    Bulk,
    HandBuilt,
}

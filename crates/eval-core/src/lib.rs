//! Functions take values and return values; the runner shell owns processes,
//! stores, clocks, and paths.

#![forbid(unsafe_code)]

/// Variant names and fields are the message; callers match on the variant.
macro_rules! debug_display {
    ($($error:ty),*) => {$(
        impl std::fmt::Display for $error {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Debug::fmt(self, f)
            }
        }
        impl std::error::Error for $error {}
    )*};
}

mod census;
mod decimal;
mod eligibility;
mod event;
mod generator;
mod identity;
mod ledger;
mod manifest;
mod markers;
mod occurrence;
mod reducer;
mod render;
mod residue;
mod stream;

pub use census::{
    Construction, HintBounds, Reachability, SURFACE1_HINT_BOUNDS, SURFACE1_STAGES, Surface1Stage,
};
pub use eligibility::*;
pub use event::*;
pub use generator::*;
pub use identity::{
    BUILD_PROTOCOL, BinaryDigest, BuildRecord, IdentityError, RUN_ID_PROTOCOL, RunIdentity,
    eval_run_id, zero_bytes_sha256,
};
pub use ledger::{
    CHAIN_STAGES, ChainStage, Completed, Evidence, Ledger, LedgerError,
    MAX_CANDIDATES_PER_STAGE_OBSERVATION, Observation, Presence, Required, Stage, StageKind,
    StageVerdict,
};
pub use manifest::{
    ArmRates, Attestation, CLAIM_BOUNDARY_EXCLUSIONS, CLAIM_BOUNDARY_SCHEMA, ClaimBoundary,
    ComponentVersions, Cut, CutOutcome, CutReceipt, DROPPED_FIELDS, ExecutionMode, Ingestion,
    MANIFEST_DIGEST_PROTOCOL, MANIFEST_SCHEMA, Manifest, ManifestError, REQUIRED_FIELDS,
    ResourceLimits, RunStatus, TokenizerProfile, is_canonical_decimal, parse_manifest,
};
pub use markers::*;
pub use occurrence::*;
pub use reducer::*;
pub use render::*;
pub use residue::{
    CLOCK_FIELD_KEEP_ALLOWLIST, ObservationSchema, ResidueEntry, ResidueError, Rule, SemanticTrace,
    TRACE_DIGEST_PROTOCOL, is_clock_named, is_never_kept,
};
pub use stream::*;

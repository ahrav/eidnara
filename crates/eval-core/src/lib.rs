//! Functions take values and return values; the runner shell owns processes,
//! stores, clocks, and paths.

#![forbid(unsafe_code)]

mod census;
mod identity;
mod manifest;
mod residue;

pub use census::{
    Construction, HintBounds, Reachability, SURFACE1_HINT_BOUNDS, SURFACE1_STAGES, Surface1Stage,
};
pub use identity::{
    BUILD_PROTOCOL, BinaryDigest, BuildRecord, IdentityError, RUN_ID_PROTOCOL, RunIdentity,
    eval_run_id, zero_bytes_sha256,
};
pub use manifest::{
    ArmRates, Attestation, CLAIM_BOUNDARY_EXCLUSIONS, CLAIM_BOUNDARY_SCHEMA, ClaimBoundary,
    ComponentVersions, Cut, CutOutcome, CutReceipt, DROPPED_FIELDS, MANIFEST_DIGEST_PROTOCOL,
    MANIFEST_SCHEMA, Manifest, ManifestError, REQUIRED_FIELDS, ResourceLimits, RunStatus,
    TokenizerProfile, is_canonical_decimal, parse_manifest,
};
pub use residue::{
    CLOCK_FIELD_KEEP_ALLOWLIST, ObservationSchema, ResidueEntry, ResidueError, Rule, SemanticTrace,
    TRACE_DIGEST_PROTOCOL, is_clock_named, is_never_kept,
};

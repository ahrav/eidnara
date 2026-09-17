//! The ranking unit is the kernel occurrence identifier, so two occurrences with equal payload bytes stay two ranking units.
//! Composite digests use length-delimited components to preserve tuple boundaries.
//!
//! No type in this module carries a project, session, or harness.
//! Parsing or constructing an identity yields bytes, never an authorization; the route binding supplies scope and compares it before any identity is admitted.

mod identity;
mod lane;
mod rrf;

pub use identity::{
    AccountingProfileIdentity, AccountingProfileRevision, ContextRepresentation, ContextRevision,
    GenerationId, IdentityRefusal, InvocationId, OccurrenceId, ParentGroupKey, ParentId,
    PreparationDigest, PreparationInputs, ProbeOrdinal, SelectedSpan, SelectionDigest,
};
pub use lane::{DeclaredLanes, Lane, LaneEntry, LaneHit, LaneRanking, RawScore};
pub use rrf::{
    Fused, FusedEntry, FusionParameters, LaneContribution, LaneWeights, ParameterRefusal,
    UnionExceeded, fuse,
};

//! The closed, versioned step schema the model answers with: one bounded read batch, one proposal, or an abstention. Decoding rejects unknown fields, unknown variants, and out-of-range values; validation then rejects aliases the broker never issued, empty or inverted ranges, and batches over the per-batch bound, all before any operation has an effect.

use std::ops::Range;

use kernel::{
    MAX_REVIEW_LIMITATIONS, MAX_REVIEW_REFERENCES, MAX_REVIEW_TEXT_BYTES, ProposalAction,
    Uncertainty,
};
use serde::Deserialize;

use super::broker::{EvidenceBroker, MAX_OPERATIONS_PER_BATCH, RefusalCode};

/// The schema version every step must declare.
pub const STEP_VERSION: u32 = 1;
/// Longest literal a search operation may carry.
pub const MAX_SEARCH_LITERAL_BYTES: usize = 256;
/// Longest relative path a project-text operation may name.
pub const MAX_PATH_BYTES: usize = 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    v: u32,
    step: Step,
}

/// One model turn.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// Up to [`MAX_OPERATIONS_PER_BATCH`] reads the model wants before it concludes.
    ReadBatch { operations: Vec<Operation> },
    /// The model's conclusion; the coordinator binds it into a proposal.
    Propose(Box<ProposedOutcome>),
    /// The model declines to conclude; `reason` is host-visible only through its length.
    Abstain { reason: String },
}

/// One bounded read the model requests. Aliases are the broker's; paths are relative literals; ranges are half-open byte ranges.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    ReadReference {
        alias: String,
        #[serde(default)]
        range: Option<ByteRange>,
    },
    FindRelated {
        #[serde(default)]
        cursor: Option<String>,
    },
    SearchProject {
        by: SearchBy,
        literal: String,
    },
    ReadProject {
        path: String,
        #[serde(default)]
        range: Option<ByteRange>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchBy {
    Path,
    Name,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    pub fn to_range(self) -> Range<u64> {
        self.start..self.end
    }
}

/// The model's proposed outcome. Citations name aliases; the coordinator resolves them to evidence. Policy dependencies and the manifest are never the model's to state.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedOutcome {
    pub action: ProposalAction,
    #[serde(default)]
    pub new_text: Option<String>,
    #[serde(default)]
    pub support: Vec<Citation>,
    #[serde(default)]
    pub contradictions: Vec<Citation>,
    #[serde(default)]
    pub limitations: Vec<String>,
    pub uncertainty: Uncertainty,
}

/// One cited alias and, optionally, the byte range inside it the claim rests on.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Citation {
    pub alias: String,
    #[serde(default)]
    pub range: Option<ByteRange>,
}

impl Step {
    /// Decodes and validates one step against `broker`'s issued aliases and the job's target kind; the refusal is the code rendered back to the model. Nothing here touches a store.
    pub fn parse(
        text: &str,
        broker: &EvidenceBroker,
        targets_memory: bool,
    ) -> Result<Self, RefusalCode> {
        let envelope: Envelope =
            serde_json::from_str(text).map_err(|_| RefusalCode::Undecodable)?;
        if envelope.v != STEP_VERSION {
            return Err(RefusalCode::Undecodable);
        }
        let step = envelope.step;
        let resolve = |alias: &str| {
            broker
                .aliases
                .resolve(alias)
                .map(|_| ())
                .map_err(|_| RefusalCode::UnknownAlias)
        };
        let range_ok = |range: Option<ByteRange>| match range {
            Some(range) if range.start >= range.end => Err(RefusalCode::InvalidRange),
            _ => Ok(()),
        };
        let literal_ok = |literal: &str, max: usize| {
            (!literal.is_empty() && literal.len() <= max)
                .then_some(())
                .ok_or(RefusalCode::TooLarge)
        };
        match &step {
            Step::ReadBatch { operations } => {
                if operations.is_empty() || operations.len() > MAX_OPERATIONS_PER_BATCH {
                    return Err(RefusalCode::BatchLimit);
                }
                for operation in operations {
                    match operation {
                        Operation::ReadReference { alias, range } => {
                            resolve(alias)?;
                            range_ok(*range)?;
                        }
                        Operation::FindRelated { .. } => {}
                        Operation::SearchProject { literal, .. } => {
                            literal_ok(literal, MAX_SEARCH_LITERAL_BYTES)?;
                        }
                        Operation::ReadProject { path, range } => {
                            literal_ok(path, MAX_PATH_BYTES)?;
                            range_ok(*range)?;
                        }
                    }
                }
            }
            Step::Propose(outcome) => {
                // The Kernel's own proposal rules (KTD2), applied before anything is bound: create needs a staged candidate and text; revise needs a memory and text; retain and retire need a memory and no text; no-change carries no text.
                let (needs_memory, needs_text) = match outcome.action {
                    ProposalAction::Create => (Some(false), true),
                    ProposalAction::Revise => (Some(true), true),
                    ProposalAction::Retain | ProposalAction::Retire => (Some(true), false),
                    ProposalAction::NoChange => (None, false),
                };
                if needs_memory.is_some_and(|needs| needs != targets_memory)
                    || needs_text != outcome.new_text.is_some()
                {
                    return Err(RefusalCode::Unsupported);
                }
                if outcome
                    .new_text
                    .as_ref()
                    .is_some_and(|text| text.is_empty() || text.len() > MAX_REVIEW_TEXT_BYTES)
                    || outcome.limitations.len() > MAX_REVIEW_LIMITATIONS
                    || outcome
                        .limitations
                        .iter()
                        .any(|text| text.is_empty() || text.len() > MAX_REVIEW_TEXT_BYTES)
                    || outcome.support.len() + outcome.contradictions.len() > MAX_REVIEW_REFERENCES
                {
                    return Err(RefusalCode::TooLarge);
                }
                for citation in outcome.support.iter().chain(&outcome.contradictions) {
                    resolve(&citation.alias)?;
                    range_ok(citation.range)?;
                }
            }
            Step::Abstain { .. } => {}
        }
        Ok(step)
    }
}

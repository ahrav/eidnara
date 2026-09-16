use std::fmt;
use std::num::{NonZeroU64, NonZeroUsize};
use std::time::Duration;

use kernel::applicability::EvalBudget;
use retrieval::packing::{OptionalBounds, RequiredBounds};
use sha2::{Digest, Sha256};

use super::render::{AccountingBounds, AccountingExceeded, Ledger, admit_render};
use super::{ClaudeTokens, CostedGroup, OptionalAdmission};
use crate::dispatch::{MAX_WIRE_BODY_BYTES, PreparedOutput, PreparedOutputError};
use crate::projection_gates::{PackingManifest, RuntimeManifest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackingLimits {
    pub fused_candidates: NonZeroUsize,
    pub payload_loads: NonZeroUsize,
    pub payload_bytes: NonZeroU64,
    pub item_bytes: NonZeroU64,
    pub parents: NonZeroUsize,
    pub spans_per_parent: NonZeroUsize,
    pub rendered_bytes: NonZeroUsize,
    pub estimated_tokens: ClaudeTokens,
    pub serialized_bytes: NonZeroUsize,
    pub adjustment_passes: usize,
    pub deadline: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackingLimitRefusal {
    Absent,
    Zero(&'static str),
    OutOfRange(&'static str),
}

fn size(value: u64, name: &'static str) -> Result<usize, PackingLimitRefusal> {
    usize::try_from(value).map_err(|_| PackingLimitRefusal::OutOfRange(name))
}

fn count(value: u64, name: &'static str) -> Result<NonZeroUsize, PackingLimitRefusal> {
    NonZeroUsize::new(size(value, name)?).ok_or(PackingLimitRefusal::Zero(name))
}

fn body_bytes(value: u64, name: &'static str) -> Result<NonZeroUsize, PackingLimitRefusal> {
    let bytes = count(value, name)?;
    if bytes.get() > MAX_WIRE_BODY_BYTES {
        return Err(PackingLimitRefusal::OutOfRange(name));
    }
    Ok(bytes)
}

fn bytes(value: u64, name: &'static str) -> Result<NonZeroU64, PackingLimitRefusal> {
    NonZeroU64::new(value).ok_or(PackingLimitRefusal::Zero(name))
}

impl PackingLimits {
    pub fn from_manifest(manifest: &RuntimeManifest) -> Result<Self, PackingLimitRefusal> {
        let PackingManifest {
            fused_candidates,
            payload_loads,
            payload_bytes,
            item_bytes,
            parents,
            spans_per_parent,
            rendered_bytes,
            estimated_tokens,
            serialized_bytes,
            adjustment_passes,
            deadline_ms,
        } = manifest.packing.ok_or(PackingLimitRefusal::Absent)?;
        Ok(Self {
            fused_candidates: count(fused_candidates, "packing_fused_candidates")?,
            payload_loads: count(payload_loads, "packing_payload_loads")?,
            payload_bytes: bytes(payload_bytes, "packing_payload_bytes")?,
            item_bytes: bytes(item_bytes, "packing_item_bytes")?,
            parents: count(parents, "packing_parents")?,
            spans_per_parent: count(spans_per_parent, "packing_spans_per_parent")?,
            rendered_bytes: body_bytes(rendered_bytes, "packing_rendered_bytes")?,
            estimated_tokens: ClaudeTokens::new(
                bytes(estimated_tokens, "packing_estimated_tokens")?.get(),
            ),
            serialized_bytes: body_bytes(serialized_bytes, "packing_serialized_bytes")?,
            adjustment_passes: size(adjustment_passes, "packing_adjustment_passes")?,
            deadline: Duration::from_millis(bytes(deadline_ms, "packing_deadline_ms")?.get()),
        })
    }

    pub fn required_bounds(&self, token_limit: ClaudeTokens) -> RequiredBounds<ClaudeTokens> {
        RequiredBounds {
            max_payload_loads: self.payload_loads,
            max_payload_bytes: self.payload_bytes,
            max_item_bytes: self.item_bytes,
            token_limit,
        }
    }

    pub fn optional_bounds(&self) -> OptionalBounds {
        OptionalBounds {
            max_fused_candidates: self.fused_candidates,
            max_parents: self.parents,
            max_spans_per_parent: self.spans_per_parent,
            max_payload_loads: self.payload_loads,
            max_payload_bytes: self.payload_bytes,
            max_item_bytes: self.item_bytes,
        }
    }

    pub fn accounting_bounds(&self) -> AccountingBounds {
        AccountingBounds {
            max_rendered_bytes: self.rendered_bytes.get(),
            max_estimated_tokens: self.estimated_tokens,
        }
    }

    pub fn serialization_bounds(&self) -> SerializationBounds {
        SerializationBounds {
            accounting: self.accounting_bounds(),
            max_serialized_bytes: self.serialized_bytes.get(),
            max_adjustment_passes: self.adjustment_passes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerializationBounds {
    pub accounting: AccountingBounds,
    pub max_serialized_bytes: usize,
    pub max_adjustment_passes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerializationBound {
    Transport,
    SerializedBytes,
    Accounting(AccountingExceeded),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackingFailure {
    AdjustmentCapExhausted {
        passes: usize,
        bound: SerializationBound,
    },
    Deadline,
    LengthMismatch {
        measured: usize,
        written: usize,
    },
    Write,
}

/// SHA-256 of the written body.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PreparationIdentity([u8; 32]);

impl PreparationIdentity {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for PreparationIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PreparationIdentity({self})")
    }
}

impl fmt::Display for PreparationIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Preparation {
    body: Vec<u8>,
    identity: PreparationIdentity,
    ledger: Ledger,
    admitted: Vec<CostedGroup>,
    removed: Vec<CostedGroup>,
    passes: usize,
}

impl Preparation {
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn identity(&self) -> PreparationIdentity {
        self.identity
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub fn admitted(&self) -> &[CostedGroup] {
        &self.admitted
    }

    pub fn removed(&self) -> &[CostedGroup] {
        &self.removed
    }

    pub fn passes(&self) -> usize {
        self.passes
    }
}

impl fmt::Debug for Preparation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Preparation")
            .field("body_len", &self.body.len())
            .field("identity", &self.identity)
            .field("admitted", &self.admitted.len())
            .field("removed", &self.removed.len())
            .field("passes", &self.passes)
            .finish()
    }
}

enum Step {
    Exceeded(SerializationBound),
    Failed(PackingFailure),
}

fn serialize(ledger: &Ledger, bounds: &SerializationBounds) -> Result<Vec<u8>, Step> {
    admit_render(ledger, &bounds.accounting)
        .map_err(|exceeded| Step::Exceeded(SerializationBound::Accounting(exceeded)))?;
    let output = PreparedOutput::cached_bytes(ledger.text().as_bytes().to_vec());
    let measured = output
        .measure()
        .map_err(|_| Step::Exceeded(SerializationBound::Transport))?;
    if measured.len() > bounds.max_serialized_bytes {
        return Err(Step::Exceeded(SerializationBound::SerializedBytes));
    }
    let mut body = Vec::with_capacity(measured.len());
    measured.write_to(&mut body).map_err(|error| {
        Step::Failed(match error {
            PreparedOutputError::LengthMismatch { measured, written } => {
                PackingFailure::LengthMismatch { measured, written }
            }
            _ => PackingFailure::Write,
        })
    })?;
    Ok(body)
}

fn rebuild(base: &Ledger, admitted: &[CostedGroup]) -> Ledger {
    let mut ledger = base.clone();
    for group in admitted {
        super::render_group(&mut ledger, group.index, &group.group);
    }
    ledger.close();
    ledger
}

pub fn finalize(
    admission: OptionalAdmission,
    bounds: &SerializationBounds,
    budget: &EvalBudget,
) -> Result<Preparation, PackingFailure> {
    let OptionalAdmission {
        base,
        mut ledger,
        mut admitted,
        ..
    } = admission;
    let mut removed = Vec::new();
    let mut passes = 0;
    loop {
        if budget.is_exhausted() {
            return Err(PackingFailure::Deadline);
        }
        let bound = match serialize(&ledger, bounds) {
            Ok(body) => {
                let identity = PreparationIdentity(Sha256::digest(&body).into());
                return Ok(Preparation {
                    body,
                    identity,
                    ledger,
                    admitted,
                    removed,
                    passes,
                });
            }
            Err(Step::Failed(failure)) => return Err(failure),
            Err(Step::Exceeded(bound)) => bound,
        };
        let exhausted = PackingFailure::AdjustmentCapExhausted { passes, bound };
        if passes >= bounds.max_adjustment_passes {
            return Err(exhausted);
        }
        let Some(last) = admitted.pop() else {
            return Err(exhausted);
        };
        removed.push(last);
        passes += 1;
        ledger = rebuild(&base, &admitted);
    }
}

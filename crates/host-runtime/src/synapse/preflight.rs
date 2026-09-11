//! Admits one text to embedding only when its admitted bytes, its exact untruncated model token count, and the certified model window all agree; nothing else reaches inference.
//!
//! The checks run in a fixed order: the lane must be ready with a verified identity, the byte bound is judged before any tokenizer work, then the exact count, then the window.
//! No step trims, normalizes, or truncates the text, and no product limit can enlarge the lane's own caps.
//! A refusal names a non-content reason; it never carries the text.

use super::embed_tokens::EmbedTokens;
use super::inference::InferenceError;
use super::{EmbeddingEngine, LaneInfo};

/// Product caps on one embedding input, narrowed to the lane's own byte cap and token window when they are wider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingInputLimits {
    pub max_bytes: usize,
    pub max_tokens: EmbedTokens,
}

impl EmbeddingInputLimits {
    /// The lane's own caps, for callers with no narrower approved limit.
    pub fn of_lane(lane: &LaneInfo) -> Self {
        Self {
            max_bytes: lane.max_text_bytes,
            max_tokens: EmbedTokens::new(lane.max_tokens),
        }
    }

    fn narrowed_to(self, lane: &LaneInfo) -> Self {
        Self {
            max_bytes: self.max_bytes.min(lane.max_text_bytes),
            max_tokens: self.max_tokens.min(EmbedTokens::new(lane.max_tokens)),
        }
    }
}

/// The verified model and tokenizer a count and a vector are bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingIdentity {
    pub model: String,
    pub fingerprint: String,
    pub table_epoch: u64,
    pub dims: usize,
}

impl EmbeddingIdentity {
    pub fn of_lane(lane: &LaneInfo) -> Self {
        Self {
            model: lane.model.clone(),
            fingerprint: lane.fingerprint.clone(),
            table_epoch: lane.table_epoch,
            dims: lane.dims,
        }
    }

    pub fn matches(&self, lane: &LaneInfo) -> bool {
        self.table_epoch == lane.table_epoch
            && self.dims == lane.dims
            && self.fingerprint == lane.fingerprint
            && self.model == lane.model
    }
}

/// One text the preflight admitted, unchanged, with the count and identity it was admitted under.
/// `embed_admitted` accepts only this type, so that path cannot skip the checks; the routed wire paths and `embed_blocking` apply their own bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedInput<'t> {
    text: &'t str,
    tokens: EmbedTokens,
    identity: EmbeddingIdentity,
}

impl<'t> AdmittedInput<'t> {
    /// The admitted text, byte for byte the text the preflight received.
    pub fn text(&self) -> &'t str {
        self.text
    }

    pub fn bytes(&self) -> usize {
        self.text.len()
    }

    pub fn tokens(&self) -> EmbedTokens {
        self.tokens
    }

    pub fn identity(&self) -> &EmbeddingIdentity {
        &self.identity
    }
}

/// The class of an inference-side failure. The message is not carried; the lane state retains it for `Artifact` and `Invariant`, and `Input` and `Execution` messages are dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceFailureKind {
    Input,
    Execution,
    Artifact,
    Invariant,
}

impl InferenceFailureKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Execution => "execution",
            Self::Artifact => "artifact",
            Self::Invariant => "invariant",
        }
    }
}

impl From<&InferenceError> for InferenceFailureKind {
    fn from(error: &InferenceError) -> Self {
        match error {
            InferenceError::Input(_) => Self::Input,
            InferenceError::Execution(_) => Self::Execution,
            InferenceError::Artifact(_) => Self::Artifact,
            InferenceError::Invariant(_) => Self::Invariant,
        }
    }
}

/// The lane state that has no verified identity to bind a count to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneUnavailableState {
    Starting,
    Disabled,
    Failing,
}

impl LaneUnavailableState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Disabled => "disabled",
            Self::Failing => "failing",
        }
    }
}

/// Why one text has no dense coverage. Every variant is free of the text itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenseUnavailable {
    /// The lane has no verified identity to bind a count to.
    LaneUnavailable { state: LaneUnavailableState },
    /// Another call holds the lane's single inference permit; the input stays admitted and can be retried.
    LaneBusy { retry_after_ms: u64 },
    /// The text has no bytes, so there is nothing to embed.
    EmptyInput,
    /// The text exceeds the byte bound; the tokenizer was never consulted.
    ByteOverflow { bytes: usize, max_bytes: usize },
    /// The lane could not produce a count.
    CountUnavailable(InferenceFailureKind),
    /// The text encodes to no tokens at all, which the model cannot embed.
    ZeroTokens { bytes: usize },
    /// The full sequence exceeds the window; nothing is truncated to make it fit.
    TokenOverflow {
        tokens: EmbedTokens,
        max_tokens: EmbedTokens,
    },
    /// The lane serving now is not the one the input was admitted under.
    IdentityChanged,
    /// Inference refused or failed the admitted input.
    Inference(InferenceFailureKind),
}

impl std::fmt::Display for DenseUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LaneUnavailable { state } => write!(f, "embedding lane is {}", state.as_str()),
            Self::LaneBusy { retry_after_ms } => {
                write!(f, "embedding lane is busy; retry after {retry_after_ms} ms")
            }
            Self::EmptyInput => f.write_str("input has no bytes"),
            Self::ByteOverflow { bytes, max_bytes } => {
                write!(f, "input is {bytes} bytes, over the {max_bytes} byte bound")
            }
            Self::CountUnavailable(kind) => {
                write!(f, "token count unavailable: {} failure", kind.as_str())
            }
            Self::ZeroTokens { bytes } => write!(f, "{bytes} bytes encode to no tokens"),
            Self::TokenOverflow { tokens, max_tokens } => {
                write!(
                    f,
                    "input is {} tokens, over the {} token window",
                    tokens.get(),
                    max_tokens.get()
                )
            }
            Self::IdentityChanged => f.write_str("embedding lane changed since admission"),
            Self::Inference(kind) => write!(f, "inference failed: {} failure", kind.as_str()),
        }
    }
}

impl std::error::Error for DenseUnavailable {}

/// Judges `text` against `limits` under `lane`, asking `engine` for the count only after the byte bound holds.
pub(super) fn admit<'t>(
    lane: &LaneInfo,
    engine: &dyn EmbeddingEngine,
    limits: EmbeddingInputLimits,
    text: &'t str,
) -> Result<AdmittedInput<'t>, DenseUnavailable> {
    let limits = limits.narrowed_to(lane);
    let bytes = text.len();
    if bytes == 0 {
        return Err(DenseUnavailable::EmptyInput);
    }
    if bytes > limits.max_bytes {
        return Err(DenseUnavailable::ByteOverflow {
            bytes,
            max_bytes: limits.max_bytes,
        });
    }
    let tokens = engine
        .untruncated_token_len(text)
        .map_err(|error| DenseUnavailable::CountUnavailable((&error).into()))?;
    if tokens.get() == 0 {
        return Err(DenseUnavailable::ZeroTokens { bytes });
    }
    if tokens > limits.max_tokens {
        return Err(DenseUnavailable::TokenOverflow {
            tokens,
            max_tokens: limits.max_tokens,
        });
    }
    Ok(AdmittedInput {
        text,
        tokens,
        identity: EmbeddingIdentity::of_lane(lane),
    })
}

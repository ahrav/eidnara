use std::fmt;

use kernel::source_identity::{
    MAX_IDENTITY_VALUE_BYTES, OccurrenceClass, Span, normalize_span_for_length, well_formed_value,
    whole_buffer_lineage_digest,
};
use sha2::{Digest, Sha256};

use crate::batch::VectorGeneration;

use super::Lane;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdentityRefusal {
    /// An uppercase or prefixed spelling would let one occurrence appear as two ranking units.
    #[error("identifier is not 64 lowercase hex characters")]
    MalformedDigest,
    #[error(
        "token is empty, longer than {MAX_IDENTITY_VALUE_BYTES} bytes, or holds a control character"
    )]
    MalformedToken,
    #[error("raw score for lane {found} was declared in a {declared} ranking")]
    ScoreLaneMismatch { declared: Lane, found: Lane },
    #[error("raw score is not finite")]
    NonFiniteScore,
    #[error("lane {0} declared more than once")]
    DuplicateLane(Lane),
    #[error("lane {lane} stamps occurrence encoding version {found}, this build mints {expected}")]
    EncodingVersion { lane: Lane, expected: u8, found: u8 },
    #[error("occurrence tuple disagrees with its derived fields")]
    TupleMismatch,
    #[error("selected span is reversed or exceeds its buffer")]
    MalformedSpan,
}

fn decode_digest(hex: &str) -> Result<[u8; 32], IdentityRefusal> {
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return Err(IdentityRefusal::MalformedDigest);
    }
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(bytes.as_chunks::<2>().0) {
        let nibble = |byte: u8| match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err(IdentityRefusal::MalformedDigest),
        };
        *slot = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Ok(out)
}

fn write_hex(bytes: &[u8; 32], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for byte in bytes {
        write!(f, "{byte:02x}")?;
    }
    Ok(())
}

macro_rules! digest_identity {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 32]);

        impl $name {
            pub fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write_hex(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}(", stringify!($name))?;
                write_hex(&self.0, f)?;
                write!(f, ")")
            }
        }
    };
}

digest_identity! {
    /// Ties resolve in byte order, so `Ord` is derived over the raw digest.
    OccurrenceId
}

impl OccurrenceId {
    /// Only the occurrence identifier is parsed from stored or wire text; every other digest is derived, so no foreign hex can become a parent, selection, or preparation.
    pub fn parse(hex: &str) -> Result<Self, IdentityRefusal> {
        decode_digest(hex).map(Self)
    }
}

digest_identity! {
    /// Every span of one source at one representation shares its parent, which is the whole-buffer lineage.
    /// A parent is a presentation unit and never a fusion voter.
    ParentId
}

digest_identity! {
    SelectionDigest
}

digest_identity! {
    PreparationDigest
}

macro_rules! token_identity {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: &str) -> Result<Self, IdentityRefusal> {
                well_formed_value(value)
                    .then(|| Self(value.to_owned()))
                    .ok_or(IdentityRefusal::MalformedToken)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

token_identity! {
    InvocationId
}

token_identity! {
    ContextRevision
}

token_identity! {
    ContextRepresentation
}

token_identity! {
    GenerationId
}

impl TryFrom<&VectorGeneration> for GenerationId {
    type Error = IdentityRefusal;

    fn try_from(generation: &VectorGeneration) -> Result<Self, IdentityRefusal> {
        Self::parse(&generation.generation_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProbeOrdinal(pub u32);

/// Spans of one source at different revisions form different groups, so a group never mixes bytes from two revisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParentGroupKey {
    parent: ParentId,
    revision: i64,
}

impl ParentGroupKey {
    /// `revision` and `span` witness the stored tuple; a derived column altered independently of the tuple is refused rather than allowed to regroup an occurrence.
    pub fn derive(
        tuple: &[u8],
        class: OccurrenceClass,
        revision: i64,
        representation: &str,
        span: Option<Span>,
    ) -> Result<Self, IdentityRefusal> {
        let parent =
            whole_buffer_lineage_digest(tuple, class.code(), revision, representation, span)
                .ok_or(IdentityRefusal::TupleMismatch)?;
        Ok(Self {
            parent: ParentId(parent),
            revision,
        })
    }

    pub fn parent(&self) -> ParentId {
        self.parent
    }

    pub fn revision(&self) -> i64 {
        self.revision
    }
}

/// `None` is the whole buffer, the same normalization the kernel applies before minting an occurrence, so a whole-buffer selection has one spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedSpan {
    occurrence: OccurrenceId,
    span: Option<Span>,
}

impl SelectedSpan {
    pub fn new(
        occurrence: OccurrenceId,
        span: Option<Span>,
        buffer_len: u64,
    ) -> Result<Self, IdentityRefusal> {
        if span.is_some_and(|span| span.start > span.end || span.end > buffer_len) {
            return Err(IdentityRefusal::MalformedSpan);
        }
        Ok(Self {
            occurrence,
            span: normalize_span_for_length(span, buffer_len),
        })
    }

    pub fn occurrence(&self) -> &OccurrenceId {
        &self.occurrence
    }

    pub fn span(&self) -> Option<Span> {
        self.span
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparationInputs<'a> {
    pub context: &'a ContextRevision,
    pub representation: &'a ContextRepresentation,
    pub spans: &'a [SelectedSpan],
    pub selection: &'a SelectionDigest,
}

/// The count of a sequence and the length of every component are hashed before their bytes, so no component can imitate a boundary.
struct Derivation(Sha256);

impl Derivation {
    fn new(domain: &'static str) -> Self {
        let mut hash = Sha256::new();
        hash.update(domain.as_bytes());
        hash.update([0u8]);
        Self(hash)
    }

    fn component(&mut self, bytes: &[u8]) {
        self.0.update((bytes.len() as u64).to_be_bytes());
        self.0.update(bytes);
    }

    fn count(&mut self, count: usize) {
        self.0.update((count as u64).to_be_bytes());
    }

    fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

impl SelectionDigest {
    pub fn derive(selection: &[OccurrenceId]) -> Self {
        let mut derivation = Derivation::new("eidnara-retrieval-selection-v1");
        derivation.count(selection.len());
        for occurrence in selection {
            derivation.component(occurrence.as_bytes());
        }
        Self(derivation.finish())
    }
}

impl PreparationDigest {
    pub fn derive(inputs: PreparationInputs<'_>) -> Self {
        let mut derivation = Derivation::new("eidnara-retrieval-preparation-v1");
        derivation.component(inputs.context.as_str().as_bytes());
        derivation.component(inputs.representation.as_str().as_bytes());
        derivation.count(inputs.spans.len());
        for selected in inputs.spans {
            derivation.component(selected.occurrence.as_bytes());
            // `0` encodes the whole buffer; `1` followed by both bounds encodes a span.
            match selected.span {
                None => derivation.component(&[0u8]),
                Some(span) => {
                    let mut range = [0u8; 17];
                    range[0] = 1;
                    range[1..9].copy_from_slice(&span.start.to_be_bytes());
                    range[9..17].copy_from_slice(&span.end.to_be_bytes());
                    derivation.component(&range);
                }
            }
        }
        derivation.component(inputs.selection.as_bytes());
        Self(derivation.finish())
    }
}

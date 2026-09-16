//! The identities fusion ranks by, groups by, selects, and binds edits to.
//!
//! The ranking unit is the kernel occurrence identifier, so two occurrences with equal payload bytes stay two ranking units.
//! Composite digests use length-delimited components to preserve tuple boundaries.
//!
//! No type in this module carries a project, session, or harness.
//! Parsing or constructing an identity yields bytes, never an authorization; the route binding supplies scope and compares it before any identity is admitted.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroUsize;

use kernel::source_identity::{OccurrenceClass, Span, derived_parent_id};
use sha2::{Digest, Sha256};

use crate::batch::VectorGeneration;

pub const MAX_TOKEN_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdentityRefusal {
    /// An uppercase or prefixed spelling would let one occurrence appear as two ranking units.
    #[error("identifier is not 64 lowercase hex characters")]
    MalformedDigest,
    #[error("token is empty, longer than {MAX_TOKEN_BYTES} bytes, or holds a control character")]
    MalformedToken,
    #[error("raw score for lane {found} was declared in a {declared} ranking")]
    ScoreLaneMismatch { declared: Lane, found: Lane },
    #[error("raw score is not finite")]
    NonFiniteScore,
    #[error("lane {0} declared more than once")]
    DuplicateLane(Lane),
    #[error("lane {lane} stamps occurrence encoding version {found}, the set uses {expected}")]
    MixedEncodingVersion { lane: Lane, expected: u8, found: u8 },
    #[error("occurrence tuple disagrees with its derived fields")]
    TupleMismatch,
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
            pub fn parse(hex: &str) -> Result<Self, IdentityRefusal> {
                decode_digest(hex).map(Self)
            }

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
    /// Byte order is the tie order every ranking uses, so `Ord` is derived over the raw digest.
    OccurrenceId
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

fn well_formed_token(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_TOKEN_BYTES && !value.chars().any(char::is_control)
}

macro_rules! token_identity {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: &str) -> Result<Self, IdentityRefusal> {
                well_formed_token(value)
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
    GenerationId
}

impl From<&VectorGeneration> for GenerationId {
    fn from(generation: &VectorGeneration) -> Self {
        Self(generation.generation_id.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProbeOrdinal(pub u32);

/// A closed set keeps a lane from being declared under two names; `ORDER` is the one summation order fusion uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lane {
    Exact,
    Lexical,
    Dense,
}

impl Lane {
    pub const ORDER: [Lane; 3] = [Lane::Exact, Lane::Lexical, Lane::Dense];

    pub fn code(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Lexical => "lexical",
            Self::Dense => "dense",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|lane| lane.code() == code)
    }
}

impl fmt::Display for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Each variant names its lane so a score can only be compared within that lane; there is no ordering across variants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawScore {
    /// The exact lane returns a set without a rank, so every member ties and the declared ranking is occurrence-identifier order.
    Exact,
    Lexical(f64),
    Dense(f64),
}

impl RawScore {
    pub fn lane(self) -> Lane {
        match self {
            Self::Exact => Lane::Exact,
            Self::Lexical(_) => Lane::Lexical,
            Self::Dense(_) => Lane::Dense,
        }
    }

    fn is_finite(self) -> bool {
        match self {
            Self::Exact => true,
            Self::Lexical(value) | Self::Dense(value) => value.is_finite(),
        }
    }

    fn better_first(self, other: Self) -> Ordering {
        match (self, other) {
            (Self::Exact, Self::Exact) => Ordering::Equal,
            (Self::Lexical(left), Self::Lexical(right)) => left.total_cmp(&right),
            (Self::Dense(left), Self::Dense(right)) => right.total_cmp(&left),
            _ => unreachable!("a lane ranking holds one score kind"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HitOrigin {
    Probe(ProbeOrdinal),
    Generation(GenerationId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaneHit {
    pub occurrence: OccurrenceId,
    pub raw_score: RawScore,
    pub origin: HitOrigin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaneEntry {
    occurrence: OccurrenceId,
    position: NonZeroUsize,
    raw_score: RawScore,
    origin: HitOrigin,
}

impl LaneEntry {
    pub fn occurrence(&self) -> &OccurrenceId {
        &self.occurrence
    }

    pub fn position(&self) -> NonZeroUsize {
        self.position
    }

    pub fn raw_score(&self) -> RawScore {
        self.raw_score
    }

    pub fn origin(&self) -> &HitOrigin {
        &self.origin
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaneRanking {
    lane: Lane,
    encoding_version: u8,
    entries: Vec<LaneEntry>,
}

impl LaneRanking {
    /// An occurrence keeps its best lane score; equal scores keep the lowest origin.
    /// Score and occurrence-identifier byte order make entry positions independent of input order.
    pub fn consolidate(
        lane: Lane,
        encoding_version: u8,
        hits: impl IntoIterator<Item = LaneHit>,
    ) -> Result<Self, IdentityRefusal> {
        let mut best: BTreeMap<OccurrenceId, (RawScore, HitOrigin)> = BTreeMap::new();
        for hit in hits {
            if hit.raw_score.lane() != lane {
                return Err(IdentityRefusal::ScoreLaneMismatch {
                    declared: lane,
                    found: hit.raw_score.lane(),
                });
            }
            if !hit.raw_score.is_finite() {
                return Err(IdentityRefusal::NonFiniteScore);
            }
            let challenger = (hit.raw_score, hit.origin);
            match best.get_mut(&hit.occurrence) {
                None => {
                    best.insert(hit.occurrence, challenger);
                }
                Some(incumbent) => {
                    if challenger
                        .0
                        .better_first(incumbent.0)
                        .then_with(|| challenger.1.cmp(&incumbent.1))
                        == Ordering::Less
                    {
                        *incumbent = challenger;
                    }
                }
            }
        }
        let mut ordered: Vec<(OccurrenceId, (RawScore, HitOrigin))> = best.into_iter().collect();
        // The BTreeMap yields identifier order; the stable sort preserves that order among equal scores.
        ordered.sort_by(|left, right| left.1.0.better_first(right.1.0));
        let entries = ordered
            .into_iter()
            .zip(1usize..)
            .map(|((occurrence, (raw_score, origin)), position)| LaneEntry {
                occurrence,
                position: NonZeroUsize::new(position).expect("positions start at one"),
                raw_score,
                origin,
            })
            .collect();
        Ok(Self {
            lane,
            encoding_version,
            entries,
        })
    }

    pub fn lane(&self) -> Lane {
        self.lane
    }

    pub fn encoding_version(&self) -> u8 {
        self.encoding_version
    }

    pub fn entries(&self) -> &[LaneEntry] {
        &self.entries
    }
}

/// Mixed encoding versions are refused because one occurrence would carry two identifiers and be counted as two ranking units.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredLanes {
    rankings: Vec<LaneRanking>,
}

impl DeclaredLanes {
    pub fn admit(mut rankings: Vec<LaneRanking>) -> Result<Self, IdentityRefusal> {
        rankings.sort_by_key(|ranking| ranking.lane);
        if let Some(pair) = rankings
            .windows(2)
            .find(|pair| pair[0].lane == pair[1].lane)
        {
            return Err(IdentityRefusal::DuplicateLane(pair[1].lane));
        }
        if let Some(first) = rankings.first()
            && let Some(other) = rankings
                .iter()
                .find(|ranking| ranking.encoding_version != first.encoding_version)
        {
            return Err(IdentityRefusal::MixedEncodingVersion {
                lane: other.lane,
                expected: first.encoding_version,
                found: other.encoding_version,
            });
        }
        Ok(Self { rankings })
    }

    pub fn rankings(&self) -> &[LaneRanking] {
        &self.rankings
    }

    pub fn lane(&self, lane: Lane) -> Option<&LaneRanking> {
        self.rankings.iter().find(|ranking| ranking.lane == lane)
    }

    pub fn encoding_version(&self) -> Option<u8> {
        self.rankings
            .first()
            .map(|ranking| ranking.encoding_version)
    }
}

/// Spans of one source at different revisions form different groups, so a group never mixes bytes from two revisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParentGroupKey {
    pub parent: ParentId,
    pub revision: i64,
}

impl ParentGroupKey {
    pub fn derive(
        tuple: &[u8],
        class: OccurrenceClass,
        revision: i64,
        representation: &str,
        span: Option<Span>,
    ) -> Result<Self, IdentityRefusal> {
        let parent = derived_parent_id(tuple, class.code(), revision, representation, span)
            .ok_or(IdentityRefusal::TupleMismatch)?;
        Ok(Self {
            parent: ParentId::parse(&parent).expect("kernel digests are lowercase hex"),
            revision,
        })
    }
}

/// `None` is the whole buffer, the same normalization the kernel applies before minting an occurrence, so a whole-buffer selection has one spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedSpan {
    pub occurrence: OccurrenceId,
    pub span: Option<Span>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparationInputs<'a> {
    pub context: &'a ContextRevision,
    pub representation: &'a str,
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
        derivation.component(inputs.representation.as_bytes());
        derivation.count(inputs.spans.len());
        for selected in inputs.spans {
            derivation.component(selected.occurrence.as_bytes());
            match selected.span {
                None => derivation.component(&[0u8]),
                Some(span) => {
                    let mut range = [1u8; 17];
                    range[1..9].copy_from_slice(&span.start.to_be_bytes());
                    range[9..].copy_from_slice(&span.end.to_be_bytes());
                    derivation.component(&range);
                }
            }
        }
        derivation.component(inputs.selection.as_bytes());
        Self(derivation.finish())
    }
}

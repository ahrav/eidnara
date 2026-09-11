//! The occurrence tuple every projection consumer keys rows by: class, native
//! identity, revision, representation, and span, encoded so no value can
//! imitate a field boundary. The encoding is the contract the construction
//! fixtures under `tests/fixtures/search-projection/` pin; changing it changes
//! the identity contract version.

use sha2::{Digest, Sha256};

/// Version byte that opens every encoded tuple.
pub const OCCURRENCE_ENCODING_VERSION: u8 = 2;
/// Role byte after the version: an occurrence tuple carries the revision, a
/// lineage does not. Hashing the two under different roles keeps a lineage id
/// from ever equalling an occurrence id.
const ROLE_OCCURRENCE: u8 = 0;
const ROLE_LINEAGE: u8 = 1;
/// Longest identity value or revision. Identity values are stable
/// identifiers, not content, and a consumer keys cache and storage entries by
/// them.
pub const MAX_IDENTITY_VALUE_BYTES: usize = 512;

/// The five source classes a projection covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OccurrenceClass {
    Messages,
    CanonicalClaims,
    PromotedMemory,
    GitCommits,
    RawToolSpans,
}

impl OccurrenceClass {
    pub const ALL: [OccurrenceClass; 5] = [
        Self::Messages,
        Self::CanonicalClaims,
        Self::PromotedMemory,
        Self::GitCommits,
        Self::RawToolSpans,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::CanonicalClaims => "canonical_claims",
            Self::PromotedMemory => "promoted_memory",
            Self::GitCommits => "git_commits",
            Self::RawToolSpans => "raw_tool_spans",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|class| class.code() == code)
    }

    /// Identity fields in tuple order.
    pub fn identity_fields(self) -> &'static [&'static str] {
        match self {
            Self::Messages => &[
                "project_id",
                "harness",
                "session_id",
                "message_id",
                "block_index",
            ],
            Self::CanonicalClaims => &["object_id"],
            Self::PromotedMemory => &["decision_object_id"],
            Self::GitCommits => &["repository_id", "object_format", "oid"],
            Self::RawToolSpans => &[
                "project_id",
                "harness",
                "session_id",
                "parent_message_id",
                "tool_call_id",
                "result_revision",
                "block_index",
            ],
        }
    }

    pub fn representations(self) -> &'static [&'static str] {
        match self {
            Self::Messages => &["text"],
            Self::CanonicalClaims => &["decision_summary", "rationale"],
            Self::PromotedMemory => &["summary"],
            Self::GitCommits => &["commit_message"],
            Self::RawToolSpans => &["tool_output", "tool_error"],
        }
    }
}

/// The harnesses whose native identities a projection accepts.
pub const HARNESSES: [&str; 2] = ["opencode", "pi"];

/// Why an occurrence was refused, in the order the checks run. The first
/// failing check names the refusal, so a record with several faults reports
/// the earliest one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceRefusal {
    UnknownClass,
    MissingIdentityField,
    UnknownIdentityField,
    /// A value is empty, longer than `MAX_IDENTITY_VALUE_BYTES`, or holds a
    /// control character.
    MalformedIdentityValue,
    UnknownHarness,
    MalformedOid,
    MissingRevision,
    /// A revision is not a canonical decimal integer, so two spellings of one
    /// number could name two occurrences.
    MalformedRevision,
    UnknownRepresentation,
    MalformedSpan,
    SpanReversed,
    SpanOutOfRange,
    SpanNotUtf8Aligned,
}

impl OccurrenceRefusal {
    pub const ALL: [OccurrenceRefusal; 13] = [
        Self::UnknownClass,
        Self::MissingIdentityField,
        Self::UnknownIdentityField,
        Self::MalformedIdentityValue,
        Self::UnknownHarness,
        Self::MalformedOid,
        Self::MissingRevision,
        Self::MalformedRevision,
        Self::UnknownRepresentation,
        Self::MalformedSpan,
        Self::SpanReversed,
        Self::SpanOutOfRange,
        Self::SpanNotUtf8Aligned,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::UnknownClass => "unknown_class",
            Self::MissingIdentityField => "missing_identity_field",
            Self::UnknownIdentityField => "unknown_identity_field",
            Self::MalformedIdentityValue => "malformed_identity_value",
            Self::UnknownHarness => "unknown_harness",
            Self::MalformedOid => "malformed_oid",
            Self::MissingRevision => "missing_revision",
            Self::MalformedRevision => "malformed_revision",
            Self::UnknownRepresentation => "unknown_representation",
            Self::MalformedSpan => "malformed_span",
            Self::SpanReversed => "span_reversed",
            Self::SpanOutOfRange => "span_out_of_range",
            Self::SpanNotUtf8Aligned => "span_not_utf8_aligned",
        }
    }
}

/// A half-open byte range on UTF-8 character boundaries within one buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u64,
    pub end: u64,
}

/// One source occurrence before it is encoded. `identity` carries the class's
/// fields as `(name, value)` pairs in any order; the set must be exactly the
/// class's fields, each once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence<'a> {
    pub class: &'a str,
    pub identity: &'a [(&'a str, &'a str)],
    pub revision: &'a str,
    pub representation: &'a str,
    /// `None` selects the whole buffer.
    pub span: Option<Span>,
}

/// The encoded tuple and the identifiers derived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedOccurrence {
    pub class: OccurrenceClass,
    /// The length-delimited tuple bytes, for a consumer to persist beside the
    /// identifier so a digest collision is settled by comparing tuples.
    pub tuple: Vec<u8>,
    /// Lowercase SHA-256 hex of `tuple`.
    pub occurrence_id: String,
    /// Identifies every revision of one source at one representation and
    /// span; the lineage a newer revision supersedes.
    pub lineage_id: String,
    /// The canonical revision number the tuple carries.
    pub revision: i64,
    pub span: Option<Span>,
}

fn push_str(out: &mut Vec<u8>, text: &str) {
    let len = u32::try_from(text.len()).expect("identity strings are bounded well below u32");
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(text.as_bytes());
}

/// Lowercase SHA-256 hex of the exact input bytes.
pub fn identity_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The digest of exact payload bytes is the payload identifier.
pub use identity_digest as payload_id;

/// Validates the span against the buffer it selects. `MalformedSpan` covers a
/// span that cannot be a byte range at all; the other three name which rule a
/// well-formed range breaks.
pub fn validate_span(span: Option<Span>, buffer: &str) -> Result<(), OccurrenceRefusal> {
    let Some(span) = span else {
        return Ok(());
    };
    let start = usize::try_from(span.start).map_err(|_| OccurrenceRefusal::MalformedSpan)?;
    let end = usize::try_from(span.end).map_err(|_| OccurrenceRefusal::MalformedSpan)?;
    if start > end {
        return Err(OccurrenceRefusal::SpanReversed);
    }
    if end > buffer.len() {
        return Err(OccurrenceRefusal::SpanOutOfRange);
    }
    if !buffer.is_char_boundary(start) || !buffer.is_char_boundary(end) {
        return Err(OccurrenceRefusal::SpanNotUtf8Aligned);
    }
    Ok(())
}

/// The bytes a span selects from `buffer`; the whole buffer for `None`. The
/// span must have passed [`validate_span`] against this buffer.
pub fn select(span: Option<Span>, buffer: &str) -> &[u8] {
    match span {
        None => buffer.as_bytes(),
        Some(span) => &buffer.as_bytes()[span.start as usize..span.end as usize],
    }
}

/// Returns `None` when `span` covers all of `buffer`, so whole-buffer
/// selections share one identifier however the producer spelled them.
pub fn normalize_span(span: Option<Span>, buffer: &str) -> Option<Span> {
    normalize_span_for_length(span, buffer.len() as u64)
}

fn normalize_span_for_length(span: Option<Span>, byte_length: u64) -> Option<Span> {
    span.filter(|span| !(span.start == 0 && span.end == byte_length))
}

pub(crate) fn well_formed_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTITY_VALUE_BYTES
        && !value.chars().any(char::is_control)
}

/// A revision is the shortest decimal spelling of a nonnegative integer that
/// fits `i64`, so one number has one spelling.
fn canonical_revision(revision: &str) -> Option<i64> {
    let value: i64 = revision.parse().ok()?;
    (value >= 0 && value.to_string() == revision).then_some(value)
}

fn finish(prefix: &[u8], role: u8, tail: &[&str], span: Option<Span>) -> Vec<u8> {
    let mut out = Vec::with_capacity(prefix.len() + 64);
    out.push(OCCURRENCE_ENCODING_VERSION);
    out.push(role);
    out.extend_from_slice(prefix);
    for text in tail {
        push_str(&mut out, text);
    }
    match span {
        None => out.push(0),
        Some(span) => {
            out.push(1);
            out.extend_from_slice(&span.start.to_be_bytes());
            out.extend_from_slice(&span.end.to_be_bytes());
        }
    }
    out
}

/// The buffer supplies span bounds and UTF-8 boundaries, not identity bytes.
/// Whole-buffer spans normalize to `None` before occurrence and lineage IDs are minted.
pub fn encode(
    occurrence: &Occurrence<'_>,
    buffer: &str,
) -> Result<EncodedOccurrence, OccurrenceRefusal> {
    let encoded = encode_metadata(occurrence, buffer.len() as u64)?;
    validate_span(occurrence.span, buffer)?;
    Ok(encoded)
}

/// Returns the lineage identifier implied by `tuple`, or `None` when the
/// supplied derived fields disagree with the tuple bytes. Readers use this to
/// detect a stored tuple-derived column altered independently of the tuple.
pub fn derived_lineage_id(
    tuple: &[u8],
    class_code: &str,
    revision: i64,
    representation: &str,
    span: Option<Span>,
) -> Option<String> {
    let mut head = vec![OCCURRENCE_ENCODING_VERSION, ROLE_OCCURRENCE];
    push_str(&mut head, class_code);
    // `finish` with an empty prefix yields version + role + tail + span;
    // stripping the two leading bytes leaves the exact tail encoding.
    let revision_text = revision.to_string();
    let tail = &finish(
        &[],
        ROLE_OCCURRENCE,
        &[&revision_text, representation],
        span,
    )[2..];
    let prefix_end = tuple.len().checked_sub(tail.len())?;
    if prefix_end < head.len() || !tuple.starts_with(&head) || &tuple[prefix_end..] != tail {
        return None;
    }
    let lineage = finish(&tuple[2..prefix_end], ROLE_LINEAGE, &[representation], span);
    Some(identity_digest(&lineage))
}

/// Callers validate span bounds and, when bytes are available, UTF-8 alignment.
pub(crate) fn encode_metadata(
    occurrence: &Occurrence<'_>,
    byte_length: u64,
) -> Result<EncodedOccurrence, OccurrenceRefusal> {
    encode_preserving_span(&Occurrence {
        span: normalize_span_for_length(occurrence.span, byte_length),
        ..*occurrence
    })
}

/// Encodes identity fields without validating or normalizing the span.
/// The producer must normalize whole-buffer spans and validate their bounds and UTF-8 alignment against the original buffer.
/// [`encode`] performs these checks when the original buffer is available.
pub fn encode_preserving_span(
    occurrence: &Occurrence<'_>,
) -> Result<EncodedOccurrence, OccurrenceRefusal> {
    let class =
        OccurrenceClass::from_code(occurrence.class).ok_or(OccurrenceRefusal::UnknownClass)?;
    let fields = class.identity_fields();
    let mut values: Vec<&str> = Vec::with_capacity(fields.len());
    for field in fields {
        let value = occurrence
            .identity
            .iter()
            .find(|(name, _)| name == field)
            .map(|(_, value)| *value)
            .ok_or(OccurrenceRefusal::MissingIdentityField)?;
        values.push(value);
    }
    // Every supplied name is a class field, and each field appears once.
    if occurrence.identity.len() != fields.len()
        || occurrence
            .identity
            .iter()
            .any(|(name, _)| !fields.contains(name))
    {
        return Err(OccurrenceRefusal::UnknownIdentityField);
    }
    if !values.iter().all(|value| well_formed_value(value)) {
        return Err(OccurrenceRefusal::MalformedIdentityValue);
    }
    if let Some(index) = fields.iter().position(|field| *field == "harness")
        && !HARNESSES.contains(&values[index])
    {
        return Err(OccurrenceRefusal::UnknownHarness);
    }
    if class == OccurrenceClass::GitCommits {
        let expected_len = match values[1] {
            "sha1" => 40,
            "sha256" => 64,
            _ => return Err(OccurrenceRefusal::MalformedOid),
        };
        let oid = values[2];
        if oid.len() != expected_len || !crate::scope::is_lower_hex_oid(oid) {
            return Err(OccurrenceRefusal::MalformedOid);
        }
    }
    if occurrence.revision.is_empty() {
        return Err(OccurrenceRefusal::MissingRevision);
    }
    let revision =
        canonical_revision(occurrence.revision).ok_or(OccurrenceRefusal::MalformedRevision)?;
    if !class.representations().contains(&occurrence.representation) {
        return Err(OccurrenceRefusal::UnknownRepresentation);
    }
    let span = occurrence.span;

    let mut prefix = Vec::new();
    push_str(&mut prefix, class.code());
    prefix.extend_from_slice(&u32::try_from(fields.len()).expect("small").to_be_bytes());
    for (field, value) in fields.iter().zip(&values) {
        push_str(&mut prefix, field);
        push_str(&mut prefix, value);
    }
    let tuple = finish(
        &prefix,
        ROLE_OCCURRENCE,
        &[occurrence.revision, occurrence.representation],
        span,
    );
    let lineage = finish(&prefix, ROLE_LINEAGE, &[occurrence.representation], span);
    Ok(EncodedOccurrence {
        class,
        occurrence_id: identity_digest(&tuple),
        lineage_id: identity_digest(&lineage),
        tuple,
        revision,
        span,
    })
}

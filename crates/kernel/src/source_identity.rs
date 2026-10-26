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

/// The six source classes a projection covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OccurrenceClass {
    Messages,
    CanonicalClaims,
    PromotedMemory,
    GitCommits,
    RawToolSpans,
    RepositoryDocuments,
}

impl OccurrenceClass {
    pub const ALL: [OccurrenceClass; 6] = [
        Self::Messages,
        Self::CanonicalClaims,
        Self::PromotedMemory,
        Self::GitCommits,
        Self::RawToolSpans,
        Self::RepositoryDocuments,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::CanonicalClaims => "canonical_claims",
            Self::PromotedMemory => "promoted_memory",
            Self::GitCommits => "git_commits",
            Self::RawToolSpans => "raw_tool_spans",
            Self::RepositoryDocuments => "repository_documents",
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
            Self::RepositoryDocuments => &["project_id", "repository_id", "relative_path"],
        }
    }

    pub fn representations(self) -> &'static [&'static str] {
        match self {
            Self::Messages => &["text"],
            Self::CanonicalClaims => &["decision_summary", "rationale"],
            Self::PromotedMemory => &["summary"],
            Self::GitCommits => &["commit_message"],
            Self::RawToolSpans => &["tool_output", "tool_error"],
            Self::RepositoryDocuments => &["text"],
        }
    }
}

/// The harnesses whose native identities a projection accepts.
pub const HARNESSES: [&str; 2] = ["opencode", "pi"];

/// The git object formats a `git_commits` identity may name, each with the
/// hex length of a complete object id.
pub const OBJECT_FORMATS: [(&str, usize); 2] = [("sha1", 40), ("sha256", 64)];

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

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// The lowercase hex spelling of `digest`, two digits per byte, as [`identity_digest`] spells a SHA-256 digest.
pub fn lower_hex(digest: &[u8]) -> String {
    let mut hex = vec![0; 2 * digest.len()];
    for (pair, &byte) in hex.as_chunks_mut::<2>().0.iter_mut().zip(digest) {
        *pair = [
            HEX_DIGITS[usize::from(byte >> 4)],
            HEX_DIGITS[usize::from(byte & 0x0f)],
        ];
    }
    String::from_utf8(hex).expect("hex digits are ASCII")
}

/// Whether `hex` is the lowercase hex spelling of `digest`.
fn spells(hex: &str, digest: &[u8]) -> bool {
    let (pairs, rest) = hex.as_bytes().as_chunks::<2>();
    pairs.len() == digest.len()
        && rest.is_empty()
        && pairs.iter().zip(digest).all(|(pair, &byte)| {
            pair[0] == HEX_DIGITS[usize::from(byte >> 4)]
                && pair[1] == HEX_DIGITS[usize::from(byte & 0x0f)]
        })
}

/// Lowercase SHA-256 hex of the exact input bytes.
pub fn identity_digest(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

/// Whether `hex` equals [`identity_digest`] of `bytes`, compared without building the string.
pub fn is_identity_digest(hex: &str, bytes: &[u8]) -> bool {
    spells(hex, &Sha256::digest(bytes))
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

pub fn normalize_span_for_length(span: Option<Span>, byte_length: u64) -> Option<Span> {
    span.filter(|span| !(span.start == 0 && span.end == byte_length))
}

pub fn well_formed_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTITY_VALUE_BYTES
        // Printable ASCII holds no control character, so only other text needs the char scan.
        && (value.bytes().all(|byte| (0x20..0x7f).contains(&byte))
            || !value.chars().any(char::is_control))
}

/// A revision is the shortest decimal spelling of a nonnegative integer that
/// fits `i64`, so one number has one spelling.
fn canonical_revision(revision: &str) -> Option<i64> {
    // A digit string without a leading zero, or "0", is the spelling of the value it parses to;
    // `parse` refuses one above `i64::MAX`.
    let digits = revision.as_bytes();
    if digits.is_empty()
        || !digits.iter().all(u8::is_ascii_digit)
        || (digits.len() > 1 && digits[0] == b'0')
    {
        return None;
    }
    revision.parse().ok()
}

fn finish(prefix: &[u8], role: u8, tail: &[&str], span: Option<Span>) -> Vec<u8> {
    let mut out = Vec::with_capacity(prefix.len() + 64);
    out.push(OCCURRENCE_ENCODING_VERSION);
    out.push(role);
    out.extend_from_slice(prefix);
    for text in tail {
        push_str(&mut out, text);
    }
    let (span_bytes, span_len) = span_encoding(span);
    out.extend_from_slice(&span_bytes[..span_len]);
    out
}

/// Bytes `push_str` writes before a string: its length as a big-endian `u32`.
const LENGTH_PREFIX_BYTES: usize = size_of::<u32>();

/// The span bytes `finish` writes last and how many of them it uses: a
/// presence flag, then the start and end offsets as big-endian `u64`s.
fn span_encoding(span: Option<Span>) -> ([u8; 17], usize) {
    let mut bytes = [0; 17];
    let Some(span) = span else {
        return (bytes, 1);
    };
    bytes[0] = 1;
    bytes[1..9].copy_from_slice(&span.start.to_be_bytes());
    bytes[9..].copy_from_slice(&span.end.to_be_bytes());
    (bytes, bytes.len())
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

fn identity_prefix<'t>(
    tuple: &'t [u8],
    class_code: &str,
    revision: i64,
    representation: &str,
    span: Option<Span>,
) -> Option<&'t [u8]> {
    // The head is version, role, and class; the tail is what `finish` writes
    // after the prefix: the revision, the representation, and the span.
    let revision_text = revision.to_string();
    let (span_bytes, span_len) = span_encoding(span);
    let tail_len = 2 * LENGTH_PREFIX_BYTES + revision_text.len() + representation.len() + span_len;
    let prefix_end = tuple.len().checked_sub(tail_len)?;
    if prefix_end < 2 + LENGTH_PREFIX_BYTES + class_code.len() {
        return None;
    }
    strip_str(
        tuple.strip_prefix(&[OCCURRENCE_ENCODING_VERSION, ROLE_OCCURRENCE])?,
        class_code,
    )?;
    let rest = strip_str(
        strip_str(&tuple[prefix_end..], &revision_text)?,
        representation,
    )?;
    (rest == &span_bytes[..span_len]).then(|| &tuple[2..prefix_end])
}

/// The bytes after `text` as `push_str` encodes it, when `bytes` starts with that encoding.
fn strip_str<'b>(bytes: &'b [u8], text: &str) -> Option<&'b [u8]> {
    let len = u32::try_from(text.len()).expect("identity strings are bounded well below u32");
    bytes
        .strip_prefix(&len.to_be_bytes())?
        .strip_prefix(text.as_bytes())
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
    let prefix = identity_prefix(tuple, class_code, revision, representation, span)?;
    Some(identity_digest(&finish(
        prefix,
        ROLE_LINEAGE,
        &[representation],
        span,
    )))
}

/// Whether `lineage_id` equals [`derived_lineage_id`] for these fields, compared without building the string.
pub fn is_derived_lineage_id(
    lineage_id: &str,
    tuple: &[u8],
    class_code: &str,
    revision: i64,
    representation: &str,
    span: Option<Span>,
) -> bool {
    identity_prefix(tuple, class_code, revision, representation, span).is_some_and(|prefix| {
        is_identity_digest(
            lineage_id,
            &finish(prefix, ROLE_LINEAGE, &[representation], span),
        )
    })
}

/// Returns the SHA-256 of the lineage encoding for `tuple`'s source and representation with no span, or `None` when derived fields do not match `tuple`.
/// `revision` and `span` only witness the tuple; neither enters the digest.
/// Every span occurrence of one source at one representation shares this value, which lets a consumer group spans under their parent without a parent column.
/// A lineage-role digest never equals an occurrence identifier.
pub fn whole_buffer_lineage_digest(
    tuple: &[u8],
    class_code: &str,
    revision: i64,
    representation: &str,
    span: Option<Span>,
) -> Option<[u8; 32]> {
    let prefix = identity_prefix(tuple, class_code, revision, representation, span)?;
    Some(Sha256::digest(finish(prefix, ROLE_LINEAGE, &[representation], None)).into())
}

/// Checks only the identity prefix.
/// Readers must separately verify the digest and revision/representation/span
/// suffix with [`identity_digest`] and [`derived_lineage_id`].
pub fn occurrence_identity_matches(
    mut tuple: &[u8],
    class: OccurrenceClass,
    identity: &[(&str, &str)],
) -> bool {
    let fields = class.identity_fields();
    if identity.len() != fields.len() {
        return false;
    }
    let mut take = |expected: &[u8]| {
        if let Some(rest) = tuple.strip_prefix(expected) {
            tuple = rest;
            true
        } else {
            false
        }
    };
    if !take(&[OCCURRENCE_ENCODING_VERSION, ROLE_OCCURRENCE])
        || !take(
            &u32::try_from(class.code().len())
                .expect("small class code")
                .to_be_bytes(),
        )
        || !take(class.code().as_bytes())
        || !take(
            &u32::try_from(fields.len())
                .expect("small field count")
                .to_be_bytes(),
        )
    {
        return false;
    }
    for (expected, (field, value)) in fields.iter().zip(identity) {
        if expected != field {
            return false;
        }
        for text in [field, value] {
            let Ok(len) = u32::try_from(text.len()) else {
                return false;
            };
            if !take(&len.to_be_bytes()) || !take(text.as_bytes()) {
                return false;
            }
        }
    }
    true
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
    let (class, prefix, revision) = validated_prefix(occurrence)?;
    let span = occurrence.span;
    let tuple = occurrence_tuple(&prefix, occurrence);
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

/// The `occurrence_id` and `revision` [`encode_preserving_span`] reports for `occurrence`, and its refusal when it refuses, without deriving the lineage.
pub fn occurrence_id_preserving_span(
    occurrence: &Occurrence<'_>,
) -> Result<(String, i64), OccurrenceRefusal> {
    let (_, prefix, revision) = validated_prefix(occurrence)?;
    Ok((
        identity_digest(&occurrence_tuple(&prefix, occurrence)),
        revision,
    ))
}

/// The `lineage_id` [`encode_preserving_span`] reports for `occurrence`, and its refusal when it refuses, without deriving the occurrence id.
fn lineage_id_preserving_span(occurrence: &Occurrence<'_>) -> Result<String, OccurrenceRefusal> {
    let (_, prefix, _) = validated_prefix(occurrence)?;
    Ok(identity_digest(&finish(
        &prefix,
        ROLE_LINEAGE,
        &[occurrence.representation],
        occurrence.span,
    )))
}

/// The `lineage_id`s [`encode_preserving_span`] reports for `occurrence` with the value of its class's last identity field set to each of `values` in turn, ending before the first value it refuses, and none when it refuses `occurrence`. The other fields are checked and encoded once, and the encoding before the varied value is hashed once.
pub fn lineage_ids_varying_last_field<'v>(
    occurrence: &Occurrence<'_>,
    values: impl IntoIterator<Item = &'v str>,
) -> Vec<String> {
    let Ok((class, prefix, _)) = validated_prefix(occurrence) else {
        return Vec::new();
    };
    let values = values.into_iter();
    let last = class.identity_fields()[class.identity_fields().len() - 1];
    if !checks_only_well_formedness(class, last) {
        return values
            .map_while(|value| {
                let identity: Vec<(&str, &str)> = occurrence
                    .identity
                    .iter()
                    .map(|&(name, current)| (name, if name == last { value } else { current }))
                    .collect();
                lineage_id_preserving_span(&Occurrence {
                    identity: &identity,
                    ..*occurrence
                })
                .ok()
            })
            .collect();
    }
    // The prefix ends with the last field's length-prefixed value, so the
    // lineage encoding splits into the bytes before that value, the value,
    // and the representation and span after it.
    let current = occurrence
        .identity
        .iter()
        .find(|(name, _)| *name == last)
        .map_or("", |(_, value)| *value);
    let encoded = finish(
        &prefix,
        ROLE_LINEAGE,
        &[occurrence.representation],
        occurrence.span,
    );
    debug_assert_eq!(&encoded[2..2 + prefix.len()], prefix.as_slice());
    let tail_len = encoded.len() - 2 - prefix.len();
    let head_len = encoded.len() - tail_len - LENGTH_PREFIX_BYTES - current.len();
    let (head, tail) = (&encoded[..head_len], &encoded[encoded.len() - tail_len..]);
    let hashed_head = Sha256::new_with_prefix(head);
    values
        .map_while(|value| {
            well_formed_value(value).then(|| {
                let mut value_bytes = Vec::with_capacity(LENGTH_PREFIX_BYTES + value.len());
                push_str(&mut value_bytes, value);
                lower_hex(
                    &hashed_head
                        .clone()
                        .chain_update(value_bytes)
                        .chain_update(tail)
                        .finalize(),
                )
            })
        })
        .collect()
}

/// Whether [`validated_prefix`] checks `field` of `class` only for well-formedness, so every well-formed value of it passes where one does: a harness must be a known harness, and a git identity's oid must fit its object format.
fn checks_only_well_formedness(class: OccurrenceClass, field: &str) -> bool {
    field != "harness" && class != OccurrenceClass::GitCommits
}

/// The occurrence tuple: the identity prefix, then the revision, the representation, and the span.
fn occurrence_tuple(prefix: &[u8], occurrence: &Occurrence<'_>) -> Vec<u8> {
    finish(
        prefix,
        ROLE_OCCURRENCE,
        &[occurrence.revision, occurrence.representation],
        occurrence.span,
    )
}

/// Runs every identity, revision, and representation check in refusal order, then returns the class, the identity prefix both the tuple and the lineage open with, and the canonical revision.
fn validated_prefix(
    occurrence: &Occurrence<'_>,
) -> Result<(OccurrenceClass, Vec<u8>, i64), OccurrenceRefusal> {
    let class =
        OccurrenceClass::from_code(occurrence.class).ok_or(OccurrenceRefusal::UnknownClass)?;
    let fields = class.identity_fields();
    // An identity supplied in tuple order names every field once; any other order is searched.
    let in_order = occurrence.identity.len() == fields.len()
        && occurrence
            .identity
            .iter()
            .zip(fields)
            .all(|((name, _), field)| name == field);
    let mut values: Vec<&str> = Vec::with_capacity(fields.len());
    if in_order {
        values.extend(occurrence.identity.iter().map(|(_, value)| *value));
    } else {
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
        let expected_len = OBJECT_FORMATS
            .iter()
            .find(|(format, _)| *format == values[1])
            .map(|(_, len)| *len)
            .ok_or(OccurrenceRefusal::MalformedOid)?;
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
    let mut prefix = Vec::with_capacity(
        2 * LENGTH_PREFIX_BYTES
            + class.code().len()
            + fields
                .iter()
                .zip(&values)
                .map(|(field, value)| 2 * LENGTH_PREFIX_BYTES + field.len() + value.len())
                .sum::<usize>(),
    );
    push_str(&mut prefix, class.code());
    prefix.extend_from_slice(&u32::try_from(fields.len()).expect("small").to_be_bytes());
    for (field, value) in fields.iter().zip(&values) {
        push_str(&mut prefix, field);
        push_str(&mut prefix, value);
    }
    Ok((class, prefix, revision))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parent_is_the_whole_buffer_lineage_shared_by_every_span_of_one_source() {
        let occurrence = |span| Occurrence {
            class: OccurrenceClass::CanonicalClaims.code(),
            identity: &[("object_id", "obj:a")],
            revision: "3",
            representation: "decision_summary",
            span,
        };
        let whole = encode_preserving_span(&occurrence(None)).unwrap();
        let head = encode_preserving_span(&occurrence(Some(Span { start: 0, end: 4 }))).unwrap();
        let tail = encode_preserving_span(&occurrence(Some(Span { start: 5, end: 9 }))).unwrap();
        let parent = |encoded: &EncodedOccurrence| {
            whole_buffer_lineage_digest(
                &encoded.tuple,
                "canonical_claims",
                encoded.revision,
                "decision_summary",
                encoded.span,
            )
            .map(|digest| identity_digest_hex(&digest))
        };
        assert_eq!(parent(&whole).as_deref(), Some(whole.lineage_id.as_str()));
        assert_eq!(parent(&head), parent(&whole));
        assert_eq!(parent(&tail), parent(&whole));
        assert_ne!(head.lineage_id, whole.lineage_id);
        for encoded in [&whole, &head, &tail] {
            assert_ne!(
                parent(encoded).as_deref(),
                Some(encoded.occurrence_id.as_str())
            );
        }
        for (revision, span) in [(4, head.span), (3, None)] {
            assert_eq!(
                whole_buffer_lineage_digest(
                    &head.tuple,
                    "canonical_claims",
                    revision,
                    "decision_summary",
                    span
                ),
                None
            );
        }
    }

    fn identity_digest_hex(digest: &[u8; 32]) -> String {
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn digest_checks_accept_exactly_the_lowercase_spelling() {
        for bytes in [&b""[..], b"a", &[0xff; 300]] {
            let hex = identity_digest(bytes);
            assert_eq!(hex, format!("{:x}", Sha256::digest(bytes)));
            assert!(is_identity_digest(&hex, bytes));
            assert!(!is_identity_digest(&hex.to_ascii_uppercase(), bytes));
            assert!(!is_identity_digest(&hex[1..], bytes));
            assert!(!is_identity_digest(&format!("{hex}0"), bytes));
            for index in 0..hex.len() {
                let mut altered = hex.clone().into_bytes();
                altered[index] = if altered[index] == b'0' { b'1' } else { b'0' };
                let altered = String::from_utf8(altered).unwrap();
                assert!(!is_identity_digest(&altered, bytes), "{index}");
            }
        }
    }

    /// `identity_prefix` as it re-encoded the head and tail to compare them.
    fn identity_prefix_by_reencoding<'t>(
        tuple: &'t [u8],
        class_code: &str,
        revision: i64,
        representation: &str,
        span: Option<Span>,
    ) -> Option<&'t [u8]> {
        let mut head = vec![OCCURRENCE_ENCODING_VERSION, ROLE_OCCURRENCE];
        push_str(&mut head, class_code);
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
        Some(&tuple[2..prefix_end])
    }

    #[test]
    fn the_identity_prefix_agrees_with_re_encoding_the_head_and_tail() {
        let spans = [
            None,
            Some(Span { start: 0, end: 0 }),
            Some(Span { start: 2, end: 7 }),
            Some(Span {
                start: u64::MAX - 1,
                end: u64::MAX,
            }),
        ];
        let oid = "c".repeat(40);
        let sources = [
            (
                OccurrenceClass::CanonicalClaims,
                vec![("object_id", "obj:\u{e9}")],
                "decision_summary",
            ),
            (
                OccurrenceClass::PromotedMemory,
                vec![("decision_object_id", "o")],
                "summary",
            ),
            (
                OccurrenceClass::GitCommits,
                vec![
                    ("repository_id", "repo"),
                    ("object_format", "sha1"),
                    ("oid", oid.as_str()),
                ],
                "commit_message",
            ),
        ];
        let mut matched = 0;
        for (class, identity, encoded_representation) in &sources {
            for (revision, span) in [
                ("0", None),
                ("12", Some(Span { start: 2, end: 7 })),
                ("9223372036854775807", None),
            ] {
                let tuple = encode_preserving_span(&Occurrence {
                    class: class.code(),
                    identity,
                    revision,
                    representation: encoded_representation,
                    span,
                })
                .unwrap()
                .tuple;
                let mut longer = tuple.clone();
                longer.push(0);
                for candidate in [&tuple[..], &tuple[..tuple.len() - 1], &longer, &tuple[..3]] {
                    for class_code in [class.code(), "messages", "git_commit", ""] {
                        for revision in [-12, 0, 12, 120, i64::MAX] {
                            for representation in ["", "summar", encoded_representation, "\u{e9}"] {
                                for span in spans {
                                    let new = identity_prefix(
                                        candidate,
                                        class_code,
                                        revision,
                                        representation,
                                        span,
                                    );
                                    let old = identity_prefix_by_reencoding(
                                        candidate,
                                        class_code,
                                        revision,
                                        representation,
                                        span,
                                    );
                                    assert_eq!(new, old);
                                    matched += usize::from(new.is_some());
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(
            matched, 9,
            "each encoded source matches exactly its own fields"
        );
    }

    #[test]
    fn lineage_checks_agree_with_the_derived_lineage() {
        for span in [None, Some(Span { start: 2, end: 7 })] {
            let encoded = encode_preserving_span(&Occurrence {
                class: OccurrenceClass::CanonicalClaims.code(),
                identity: &[("object_id", "obj:a")],
                revision: "12",
                representation: "decision_summary",
                span,
            })
            .unwrap();
            let derived = |revision, representation, span| {
                let id = derived_lineage_id(
                    &encoded.tuple,
                    "canonical_claims",
                    revision,
                    representation,
                    span,
                );
                let checked = is_derived_lineage_id(
                    &encoded.lineage_id,
                    &encoded.tuple,
                    "canonical_claims",
                    revision,
                    representation,
                    span,
                );
                assert_eq!(checked, id.as_deref() == Some(encoded.lineage_id.as_str()));
                id
            };
            assert_eq!(
                derived(12, "decision_summary", span).as_deref(),
                Some(encoded.lineage_id.as_str())
            );
            let moved = Some(Span { start: 2, end: 8 });
            for (revision, representation, span) in [
                (11, "decision_summary", span),
                (120, "decision_summary", span),
                (12, "rationale", span),
                (12, "decision_summar", span),
                (
                    12,
                    "decision_summary",
                    if span.is_some() { None } else { moved },
                ),
                (
                    12,
                    "decision_summary",
                    if span.is_some() { moved } else { None },
                ),
            ] {
                if (revision, representation, span) != (12, "decision_summary", encoded.span) {
                    assert_eq!(derived(revision, representation, span), None);
                }
            }
            assert!(!is_derived_lineage_id(
                &encoded.lineage_id,
                &encoded.tuple[..encoded.tuple.len() - 1],
                "canonical_claims",
                12,
                "decision_summary",
                span
            ));
        }
    }

    /// An identity in any order encodes as it does in tuple order, and a repeated, extra, misspelled, or missing name is refused by the first check it fails.
    #[test]
    fn identity_order_and_names_decide_the_refusal() {
        let in_order = [
            ("project_id", "p"),
            ("harness", "pi"),
            ("session_id", "s"),
            ("parent_message_id", "m"),
            ("tool_call_id", "call"),
            ("result_revision", "17"),
            ("block_index", "0"),
        ];
        let encode = |identity: &[(&str, &str)]| {
            occurrence_id_preserving_span(&Occurrence {
                class: "raw_tool_spans",
                identity,
                revision: "17",
                representation: "tool_output",
                span: None,
            })
        };
        let mut reversed = in_order;
        reversed.reverse();
        assert!(encode(&in_order).is_ok());
        assert_eq!(encode(&reversed), encode(&in_order));
        let mut repeated = in_order;
        repeated[1] = repeated[0];
        assert_eq!(
            encode(&repeated),
            Err(OccurrenceRefusal::MissingIdentityField)
        );
        let mut extra = in_order.to_vec();
        extra.push(("object_id", "o"));
        assert_eq!(encode(&extra), Err(OccurrenceRefusal::UnknownIdentityField));
        let mut misspelled = in_order;
        misspelled[3] = ("parent_id", "m");
        assert_eq!(
            encode(&misspelled),
            Err(OccurrenceRefusal::MissingIdentityField)
        );
        let mut missing_and_malformed = in_order[1..].to_vec();
        missing_and_malformed[0] = ("harness", "");
        assert_eq!(
            encode(&missing_and_malformed),
            Err(OccurrenceRefusal::MissingIdentityField)
        );
        let mut malformed = in_order;
        malformed[2] = ("session_id", "s\u{85}");
        assert_eq!(
            encode(&malformed),
            Err(OccurrenceRefusal::MalformedIdentityValue)
        );
    }

    /// Identity values refuse every control character, ASCII, DEL, or C1, and accept other text; a revision is accepted only in the shortest decimal spelling of a nonnegative `i64`.
    #[test]
    fn values_and_revisions_keep_their_spelling_rules() {
        for (value, accepted) in [
            ("abc~ !", true),
            ("\u{e9}", true),
            ("\u{a0}", true),
            ("\u{65e5}\u{672c}", true),
            ("", false),
            ("tab\t", false),
            ("\u{7f}", false),
            ("a\u{80}", false),
            ("\u{85}", false),
            ("\u{9f}z", false),
            ("ab\u{85}", false),
        ] {
            assert_eq!(well_formed_value(value), accepted, "{value:?}");
        }
        assert!(well_formed_value(&"y".repeat(MAX_IDENTITY_VALUE_BYTES)));
        assert!(!well_formed_value(
            &"y".repeat(MAX_IDENTITY_VALUE_BYTES + 1)
        ));
        for (revision, value) in [
            ("0", Some(0)),
            ("7", Some(7)),
            ("1780000000000", Some(1_780_000_000_000)),
            ("9223372036854775807", Some(i64::MAX)),
            ("9223372036854775808", None),
            ("01", None),
            ("00", None),
            ("+1", None),
            ("-0", None),
            ("-1", None),
            ("", None),
            ("1.5", None),
            (" 1", None),
            ("1 ", None),
            ("\u{661}", None),
        ] {
            assert_eq!(canonical_revision(revision), value, "{revision:?}");
        }
    }

    /// The lineage-free identifier is the full encoder's `occurrence_id` and `revision`, the occurrence-free identifier is its `lineage_id`, and each refuses exactly where it refuses; varying the last identity field yields the lineage ids encoded one value at a time, up to the first refused value.
    #[test]
    fn the_occurrence_id_agrees_with_the_full_encoding() {
        let oid = "b".repeat(40);
        let long = "v".repeat(MAX_IDENTITY_VALUE_BYTES + 1);
        let cases = [
            (
                "canonical_claims",
                vec![("object_id", "obj:\u{e9}")],
                "3",
                "rationale",
            ),
            (
                "messages",
                vec![
                    ("project_id", "p"),
                    ("harness", "pi"),
                    ("session_id", "s"),
                    ("message_id", "m"),
                    ("block_index", "3"),
                ],
                "1",
                "text",
            ),
            (
                "messages",
                vec![
                    ("block_index", "3"),
                    ("message_id", "m"),
                    ("session_id", "s"),
                    ("harness", "opencode"),
                    ("project_id", "p"),
                ],
                "1",
                "text",
            ),
            (
                "raw_tool_spans",
                vec![
                    ("project_id", "p"),
                    ("harness", "pi"),
                    ("session_id", "s"),
                    ("parent_message_id", "m"),
                    ("tool_call_id", "call|item"),
                    ("result_revision", "17"),
                    ("block_index", "0"),
                ],
                "17",
                "tool_error",
            ),
            (
                "git_commits",
                vec![
                    ("repository_id", "r"),
                    ("object_format", "sha1"),
                    ("oid", oid.as_str()),
                ],
                "0",
                "commit_message",
            ),
            ("nope", vec![("object_id", "o")], "1", "summary"),
            ("promoted_memory", vec![], "1", "summary"),
            (
                "promoted_memory",
                vec![("decision_object_id", "a\u{85}")],
                "1",
                "summary",
            ),
            (
                "promoted_memory",
                vec![("decision_object_id", long.as_str())],
                "1",
                "summary",
            ),
            (
                "promoted_memory",
                vec![("decision_object_id", "o")],
                "01",
                "summary",
            ),
            (
                "promoted_memory",
                vec![("decision_object_id", "o")],
                "1",
                "text",
            ),
        ];
        for (class, identity, revision, representation) in &cases {
            for span in [None, Some(Span { start: 1, end: 2 })] {
                let occurrence = Occurrence {
                    class,
                    identity,
                    revision,
                    representation,
                    span,
                };
                assert_eq!(
                    occurrence_id_preserving_span(&occurrence),
                    encode_preserving_span(&occurrence)
                        .map(|encoded| (encoded.occurrence_id, encoded.revision)),
                    "{occurrence:?}"
                );
                assert_eq!(
                    lineage_id_preserving_span(&occurrence),
                    encode_preserving_span(&occurrence).map(|encoded| encoded.lineage_id),
                    "{occurrence:?}"
                );
                let Some(class) = OccurrenceClass::from_code(class) else {
                    continue;
                };
                let last = class.identity_fields()[class.identity_fields().len() - 1];
                let values = ["0", "17", "pi", "", "\u{7}", long.as_str(), "9"];
                for start in 0..values.len() {
                    let one_by_one: Vec<String> = values[start..]
                        .iter()
                        .map_while(|value| {
                            let identity: Vec<(&str, &str)> = identity
                                .iter()
                                .map(|&(name, current)| {
                                    (name, if name == last { *value } else { current })
                                })
                                .collect();
                            lineage_id_preserving_span(&Occurrence {
                                identity: &identity,
                                ..occurrence
                            })
                            .ok()
                        })
                        .collect();
                    let expected = if lineage_id_preserving_span(&occurrence).is_ok() {
                        one_by_one
                    } else {
                        Vec::new()
                    };
                    assert_eq!(
                        lineage_ids_varying_last_field(
                            &occurrence,
                            values[start..].iter().copied()
                        ),
                        expected,
                        "{occurrence:?} from {start}"
                    );
                }
            }
        }
    }

    #[test]
    fn identity_matching_requires_the_complete_exact_prefix() {
        let oid = "a".repeat(40);
        for (class, identity, representation) in [
            (
                OccurrenceClass::CanonicalClaims,
                vec![("object_id", "obj:é")],
                "decision_summary",
            ),
            (
                OccurrenceClass::PromotedMemory,
                vec![("decision_object_id", "obj:é")],
                "summary",
            ),
            (
                OccurrenceClass::GitCommits,
                vec![
                    ("repository_id", "repo:a"),
                    ("object_format", "sha1"),
                    ("oid", &oid),
                ],
                "commit_message",
            ),
        ] {
            let tuple = encode_preserving_span(&Occurrence {
                class: class.code(),
                identity: &identity,
                revision: "1",
                representation,
                span: None,
            })
            .unwrap()
            .tuple;
            let prefix_len = 2
                + 4
                + class.code().len()
                + 4
                + identity
                    .iter()
                    .map(|(field, value)| 8 + field.len() + value.len())
                    .sum::<usize>();
            for end in 0..=tuple.len() {
                assert_eq!(
                    occurrence_identity_matches(&tuple[..end], class, &identity),
                    end >= prefix_len
                );
            }
            for index in 0..prefix_len {
                let mut damaged = tuple.clone();
                damaged[index] ^= 1;
                assert!(
                    !occurrence_identity_matches(&damaged, class, &identity),
                    "byte {index}"
                );
            }
            assert!(!occurrence_identity_matches(&tuple, class, &[]));
            let mut wrong = identity.clone();
            wrong[0].1 = "elsewhere";
            assert!(!occurrence_identity_matches(&tuple, class, &wrong));
            wrong[0] = ("wrong_field", identity[0].1);
            assert!(!occurrence_identity_matches(&tuple, class, &wrong));
        }
    }
}

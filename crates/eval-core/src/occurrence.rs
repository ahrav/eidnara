use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The kernel's `OCCURRENCE_ENCODING_VERSION`; a kernel test pins the two equal.
pub const OCCURRENCE_ENCODING_VERSION: u8 = 2;
pub const IDENTITY_CONTRACT_VERSION: &str = "search-projection-identity-v3";
pub const MAX_IDENTITY_VALUE_BYTES: usize = 512;
const ROLE_OCCURRENCE: u8 = 0;
const ROLE_LINEAGE: u8 = 1;
pub const HARNESSES: [&str; 2] = ["opencode", "pi"];
pub const OBJECT_FORMATS: [(&str, usize); 2] = [("sha1", 40), ("sha256", 64)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceClass {
    Messages,
    CanonicalClaims,
    PromotedMemory,
    GitCommits,
    RawToolSpans,
}

impl OccurrenceClass {
    pub fn code(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::CanonicalClaims => "canonical_claims",
            Self::PromotedMemory => "promoted_memory",
            Self::GitCommits => "git_commits",
            Self::RawToolSpans => "raw_tool_spans",
        }
    }

    pub const ALL: [Self; 5] = [
        Self::Messages,
        Self::CanonicalClaims,
        Self::PromotedMemory,
        Self::GitCommits,
        Self::RawToolSpans,
    ];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u64,
    pub end: u64,
}

/// A source occurrence as the encoder reads it: identity values in any order,
/// the revision in its canonical decimal spelling, and an optional span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence<'a> {
    pub class: &'a str,
    pub identity: &'a [(&'a str, &'a str)],
    pub revision: &'a str,
    pub representation: &'a str,
    pub span: Option<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub occurrence_id: String,
    pub lineage_id: String,
}

/// Refusals in the order the checks run; the earliest fault names the refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncodingRefusal {
    UnknownClass,
    MissingIdentityField,
    UnknownIdentityField,
    MalformedIdentityValue,
    UnknownHarness,
    MalformedOid,
    MissingRevision,
    MalformedRevision,
    UnknownRepresentation,
}

debug_display!(EncodingRefusal);

fn push_str(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as u32).to_be_bytes());
    out.extend_from_slice(text.as_bytes());
}

fn finish(prefix: &[u8], role: u8, tail: &[&str], span: Option<Span>) -> String {
    let mut out = vec![OCCURRENCE_ENCODING_VERSION, role];
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
    format!("{:x}", Sha256::digest(&out))
}

/// The versioned copy of the kernel's identity rule: SHA-256 over class,
/// identity fields in class order, revision, representation, and span. Payload
/// bytes never enter; a whole-buffer span is the caller's to normalize away.
pub fn encode(occurrence: &Occurrence<'_>) -> Result<Identity, EncodingRefusal> {
    let class =
        OccurrenceClass::from_code(occurrence.class).ok_or(EncodingRefusal::UnknownClass)?;
    let fields = class.identity_fields();
    let mut values = Vec::with_capacity(fields.len());
    for field in fields {
        let (_, value) = occurrence
            .identity
            .iter()
            .find(|(name, _)| name == field)
            .ok_or(EncodingRefusal::MissingIdentityField)?;
        values.push(*value);
    }
    if occurrence.identity.len() != fields.len()
        || occurrence
            .identity
            .iter()
            .any(|(name, _)| !fields.contains(name))
    {
        return Err(EncodingRefusal::UnknownIdentityField);
    }
    let well_formed = |value: &str| {
        !value.is_empty()
            && value.len() <= MAX_IDENTITY_VALUE_BYTES
            && !value.chars().any(char::is_control)
    };
    if !values.iter().all(|value| well_formed(value)) {
        return Err(EncodingRefusal::MalformedIdentityValue);
    }
    if let Some(index) = fields.iter().position(|field| *field == "harness")
        && !HARNESSES.contains(&values[index])
    {
        return Err(EncodingRefusal::UnknownHarness);
    }
    if class == OccurrenceClass::GitCommits {
        let (_, len) = OBJECT_FORMATS
            .iter()
            .find(|(format, _)| *format == values[1])
            .ok_or(EncodingRefusal::MalformedOid)?;
        let oid = values[2];
        if oid.len() != *len || !oid.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(EncodingRefusal::MalformedOid);
        }
    }
    if occurrence.revision.is_empty() {
        return Err(EncodingRefusal::MissingRevision);
    }
    let canonical = |value: &i64| *value >= 0 && value.to_string() == occurrence.revision;
    if !occurrence
        .revision
        .parse::<i64>()
        .is_ok_and(|value| canonical(&value))
    {
        return Err(EncodingRefusal::MalformedRevision);
    }
    if !class.representations().contains(&occurrence.representation) {
        return Err(EncodingRefusal::UnknownRepresentation);
    }
    let mut prefix = Vec::new();
    push_str(&mut prefix, class.code());
    prefix.extend_from_slice(&(fields.len() as u32).to_be_bytes());
    for (field, value) in fields.iter().zip(&values) {
        push_str(&mut prefix, field);
        push_str(&mut prefix, value);
    }
    let representation = occurrence.representation;
    Ok(Identity {
        occurrence_id: finish(
            &prefix,
            ROLE_OCCURRENCE,
            &[occurrence.revision, representation],
            occurrence.span,
        ),
        lineage_id: finish(&prefix, ROLE_LINEAGE, &[representation], occurrence.span),
    })
}

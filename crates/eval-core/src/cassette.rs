//! Model I/O replay keyed by canonical request digests. A request whose
//! digest matches no unconsumed entry is a typed miss, and the cassette serves
//! no later entries after a request-digest mismatch. Equal digests replay in
//! recorded order, so concurrent agent-loop requests (a title request racing
//! the main turn) replay whatever order they arrive in.
//!
//! Two provider boundaries share one schema version with two covered-field
//! lists: [`OpenCodeRequest`] is authoritative for agent-loop replay, and
//! [`BackendRecord`] covers daemon single-shot calls.

use std::collections::BTreeMap;

use context_core::canonical_json::{ContractError, canonical_json_encode, protocol_digest};
use context_core::redaction::{RedactionErrorKind, Redactor};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::manifest::is_canonical_decimal;

pub const CASSETTE_SCHEMA: &str = "eval-cassette/v1";
pub const CASSETTE_GENERATOR_VERSION: &str = "eval-cassette-rust-v1";
pub const CASSETTE_REQUEST_DIGEST_PROTOCOL: &str = "eval-cassette-request/v1";
pub const CASSETTE_PROVENANCE_PROTOCOL: &str = "eval-cassette-file/v1";
/// Names the two covered-field lists, the header allowlist, and the volatile
/// rule in this module; a cassette recorded under another version is refused.
pub const COVERED_FIELDS_VERSION: &str = "eval-cassette-covered/v1";

/// The OpenCode provider request fields the digest covers, sorted. Observed
/// from OpenCode 1.18.31 through `@ai-sdk/anthropic`: the body carries
/// `model`, `max_tokens`, `messages` (tool results travel as `tool_result`
/// blocks inside user messages), `system` blocks, `tools`, `tool_choice`, and
/// `stream`; `temperature` appears only when configured; the headers carry
/// `anthropic-version` and no beta header by default. Every body field is
/// covered, so a body field outside this list is refused rather than ignored.
pub const OPENCODE_COVERED_FIELDS: [&str; 11] = [
    "body.max_tokens",
    "body.messages",
    "body.model",
    "body.stream",
    "body.system",
    "body.temperature",
    "body.tool_choice",
    "body.tools",
    "headers.anthropic-beta",
    "headers.anthropic-version",
    "path",
];

/// Request headers a cassette retains. Every other header (`x-api-key`,
/// `authorization`, `user-agent`, `x-session-id`, `host`, `content-length`) is
/// connection- or credential-bearing and is dropped before digesting and
/// before persistence.
pub const OPENCODE_HEADER_ALLOWLIST: [&str; 2] = ["anthropic-beta", "anthropic-version"];

/// `BackendRequest` fields the digest covers, sorted: every declaration except
/// `run_id` (a per-incarnation counter) and `session` (a per-run identity, so
/// covering it would miss on every replay).
pub const BACKEND_COVERED_FIELDS: [&str; 7] = [
    "harness",
    "max_output_tokens",
    "model",
    "prompt",
    "provider",
    "system",
    "temperature",
];

/// Which provider boundary an entry was recorded at; a lookup answers only
/// from entries of its own boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    Opencode,
    Backend,
}

/// One OpenCode provider request as the mock sees it: path, request headers,
/// and the JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeRequest {
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}

/// The covered projection of a `BackendRequest`. `temperature` is the exact
/// decimal text of the `f64` ([`canonical_decimal_f64`]), or absent when the
/// request leaves the provider's native decoding in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendRecord {
    pub prompt: String,
    pub system: Option<String>,
    pub provider: String,
    pub model: String,
    pub max_output_tokens: u64,
    pub temperature: Option<String>,
    pub harness: String,
}

/// The shortest round-trip decimal of a finite, non-negative `f64`, so `0.7`
/// and `0.70` share one encoding and `0.7` and `0.8` do not.
pub fn canonical_decimal_f64(value: f64) -> Result<String, CassetteError> {
    let text = value.to_string();
    if value.is_finite() && is_canonical_decimal(&text) {
        Ok(text)
    } else {
        Err(CassetteError::TemperatureNotDecimal(text))
    }
}

/// Which covered content changed between the nearest unconsumed entry and the
/// offered request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MissClass {
    /// Only `tool_result` block contents differ.
    ToolResultDrift,
    ModelRequestChanged,
}

/// The typed terminal a strict miss produces. The run stops here; every later
/// lookup returns the same miss. `turn` counts the lookups made so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CassetteMiss {
    pub turn: usize,
    pub class: MissClass,
    pub request_digest: String,
    /// The first unconsumed entry of the lookup's boundary, or the last entry
    /// when every entry is consumed; `None` for an empty cassette.
    pub nearest_recorded: Option<String>,
}

/// `WrongNamespace` is a lookup or record under a namespace other than the
/// cassette's; `ProvenanceMismatch` means `input_sha256` does not equal the
/// digest recomputed over the file; `EntryDigestMismatch` names an entry whose
/// stored digest is not the digest of its stored request; `RedactionRefused`
/// names a secret, or content the scanner cannot finish, in `location`, and
/// the entry is refused whole rather than placeholder-substituted, after which
/// the cassette refuses to persist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CassetteError {
    SchemaMismatch { found: String },
    GeneratorVersionMismatch { found: String },
    CoveredFieldsMismatch { found: String },
    NamespaceMismatch { recorded: String, expected: String },
    WrongNamespace { recorded: String, offered: String },
    ProvenanceMismatch { recorded: String, computed: String },
    EntryDigestMismatch { index: usize },
    Shape(String),
    MalformedBody,
    UnknownRequestField(String),
    TemperatureNotDecimal(String),
    RedactionRefused(Location, RedactionErrorKind),
    ScannerUnavailable(RedactionErrorKind),
    RecordOnReplay,
    LookupOnRecord,
    NotCanonical(ContractError),
}

debug_display!(CassetteError);

impl CassetteError {
    /// The wire name the oracle reports; the TypeScript client compares these
    /// strings, so a variant rename is a protocol change.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SchemaMismatch { .. } => "SchemaMismatch",
            Self::GeneratorVersionMismatch { .. } => "GeneratorVersionMismatch",
            Self::CoveredFieldsMismatch { .. } => "CoveredFieldsMismatch",
            Self::NamespaceMismatch { .. } => "NamespaceMismatch",
            Self::WrongNamespace { .. } => "WrongNamespace",
            Self::ProvenanceMismatch { .. } => "ProvenanceMismatch",
            Self::EntryDigestMismatch { .. } => "EntryDigestMismatch",
            Self::Shape(_) => "Shape",
            Self::MalformedBody => "MalformedBody",
            Self::UnknownRequestField(_) => "UnknownRequestField",
            Self::TemperatureNotDecimal(_) => "TemperatureNotDecimal",
            Self::RedactionRefused(..) => "RedactionRefused",
            Self::ScannerUnavailable(_) => "ScannerUnavailable",
            Self::RecordOnReplay => "RecordOnReplay",
            Self::LookupOnRecord => "LookupOnRecord",
            Self::NotCanonical(_) => "NotCanonical",
        }
    }

    /// The wire detail the oracle reports beside `kind`. Request-derived and
    /// input-quoting serde payloads are withheld so `detail` never echoes
    /// request content.
    pub fn detail(&self) -> String {
        match self {
            Self::SchemaMismatch { .. }
            | Self::GeneratorVersionMismatch { .. }
            | Self::CoveredFieldsMismatch { .. }
            | Self::NamespaceMismatch { .. }
            | Self::WrongNamespace { .. }
            | Self::ProvenanceMismatch { .. }
            | Self::EntryDigestMismatch { .. }
            | Self::RedactionRefused(..)
            | Self::ScannerUnavailable(_)
            | Self::RecordOnReplay
            | Self::LookupOnRecord => self.to_string(),
            Self::Shape(_)
            | Self::MalformedBody
            | Self::UnknownRequestField(_)
            | Self::TemperatureNotDecimal(_)
            | Self::NotCanonical(_) => String::new(),
        }
    }
}

impl From<ContractError> for CassetteError {
    fn from(error: ContractError) -> Self {
        Self::NotCanonical(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Location {
    Request,
    Response,
}

/// One recorded exchange. `request` is the covered projection (redacted and
/// volatile-stripped); `response` is opaque to the core and interpreted by the
/// boundary that recorded it (SSE frames and status for OpenCode, events and a
/// terminal for the backend).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub boundary: Boundary,
    pub request_digest: String,
    pub request: Value,
    pub response: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub generator_version: String,
    pub input_sha256: String,
}

/// The persisted form. `declarations` carries what the recorded backend
/// declared per harness (`unavailable_reason`, `context_capabilities`) so a
/// replaying backend answers the same; the core does not interpret it.
/// `input_sha256` covers every field but `provenance`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CassetteFile {
    pub schema: String,
    pub namespace: String,
    pub covered_fields_version: String,
    pub declarations: Value,
    pub provenance: Provenance,
    pub cases: Vec<Entry>,
}

impl CassetteFile {
    /// The `input_sha256` this file's contents demand.
    pub fn input_sha256(&self) -> Result<String, CassetteError> {
        let covered = serde_json::json!({
            "schema": self.schema,
            "namespace": self.namespace,
            "covered_fields_version": self.covered_fields_version,
            "declarations": self.declarations,
            "cases": self.cases,
        });
        Ok(protocol_digest(CASSETTE_PROVENANCE_PROTOCOL, &covered)?)
    }
}

/// The outcome of one strict lookup. A miss is an expected replay result, not
/// a refusal; refusals are the `Err` of the enclosing `Result`.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup<'a> {
    Hit(&'a Entry),
    Miss(CassetteMiss),
}

/// A recording or replaying cassette with its consumption state.
pub struct Cassette {
    namespace: String,
    declarations: Value,
    cases: Vec<Entry>,
    consumed: Vec<bool>,
    lookups: usize,
    terminal: Option<CassetteMiss>,
    /// `Some` while recording; the scanner every entry passes through.
    redactor: Option<Redactor>,
    refused: Option<CassetteError>,
}

impl Cassette {
    /// An empty recording cassette bound to `namespace`.
    pub fn recording(namespace: &str, declarations: Value) -> Result<Self, CassetteError> {
        let redactor =
            Redactor::new().map_err(|error| CassetteError::ScannerUnavailable(error.kind()))?;
        Ok(Self {
            namespace: namespace.to_string(),
            declarations,
            cases: Vec::new(),
            consumed: Vec::new(),
            lookups: 0,
            terminal: None,
            redactor: Some(redactor),
            refused: None,
        })
    }

    /// Loads a persisted cassette for replay under `namespace`, refusing a
    /// schema, generator, covered-field, namespace, provenance, or entry-digest
    /// mismatch before any request is served.
    pub fn replay(value: &Value, namespace: &str) -> Result<Self, CassetteError> {
        let file = CassetteFile::deserialize(value)
            .map_err(|error| CassetteError::Shape(error.to_string()))?;
        if file.schema != CASSETTE_SCHEMA {
            return Err(CassetteError::SchemaMismatch { found: file.schema });
        }
        if file.provenance.generator_version != CASSETTE_GENERATOR_VERSION {
            return Err(CassetteError::GeneratorVersionMismatch {
                found: file.provenance.generator_version,
            });
        }
        if file.covered_fields_version != COVERED_FIELDS_VERSION {
            return Err(CassetteError::CoveredFieldsMismatch {
                found: file.covered_fields_version,
            });
        }
        if file.namespace != namespace {
            return Err(CassetteError::NamespaceMismatch {
                recorded: file.namespace,
                expected: namespace.to_string(),
            });
        }
        let computed = file.input_sha256()?;
        if computed != file.provenance.input_sha256 {
            return Err(CassetteError::ProvenanceMismatch {
                recorded: file.provenance.input_sha256,
                computed,
            });
        }
        for (index, entry) in file.cases.iter().enumerate() {
            if request_digest(&entry.request)? != entry.request_digest {
                return Err(CassetteError::EntryDigestMismatch { index });
            }
        }
        Ok(Self {
            namespace: file.namespace,
            declarations: file.declarations,
            consumed: vec![false; file.cases.len()],
            cases: file.cases,
            lookups: 0,
            terminal: None,
            redactor: None,
            refused: None,
        })
    }

    pub fn declarations(&self) -> &Value {
        &self.declarations
    }

    pub fn cases(&self) -> &[Entry] {
        &self.cases
    }

    /// Recorded entries no lookup has consumed; a faithful replay leaves none.
    pub fn unconsumed(&self) -> usize {
        self.consumed.iter().filter(|done| !**done).count()
    }

    /// Misses this cassette has produced: 0, or 1 once the terminal is set.
    pub fn misses(&self) -> usize {
        usize::from(self.terminal.is_some())
    }

    pub fn terminal(&self) -> Option<&CassetteMiss> {
        self.terminal.as_ref()
    }

    /// The persisted form with provenance recomputed. A cassette that refused
    /// an entry refuses to persist at all, so a partial recording never looks
    /// complete.
    pub fn to_file(&self) -> Result<CassetteFile, CassetteError> {
        if let Some(refused) = &self.refused {
            return Err(refused.clone());
        }
        let mut file = CassetteFile {
            schema: CASSETTE_SCHEMA.to_string(),
            namespace: self.namespace.clone(),
            covered_fields_version: COVERED_FIELDS_VERSION.to_string(),
            declarations: self.declarations.clone(),
            provenance: Provenance {
                generator_version: CASSETTE_GENERATOR_VERSION.to_string(),
                input_sha256: String::new(),
            },
            cases: self.cases.clone(),
        };
        file.provenance.input_sha256 = file.input_sha256()?;
        Ok(file)
    }

    /// Admits one exchange. The covered request projection and the response
    /// are scanned first; a finding or an unscannable text refuses the entry
    /// before it exists anywhere and marks the cassette refused.
    pub fn record(
        &mut self,
        namespace: &str,
        boundary: Boundary,
        request: Value,
        response: Value,
    ) -> Result<&Entry, CassetteError> {
        let redactor = self
            .redactor
            .as_ref()
            .ok_or(CassetteError::RecordOnReplay)?;
        self.check_namespace(namespace)?;
        if let Err(error) = admit(redactor, Location::Request, &request)
            .and_then(|()| admit(redactor, Location::Response, &response))
        {
            self.refused = Some(error.clone());
            return Err(error);
        }
        let entry = Entry {
            boundary,
            request_digest: request_digest(&request)?,
            request,
            response,
        };
        self.cases.push(entry);
        self.consumed.push(true);
        Ok(self.cases.last().expect("pushed"))
    }

    /// Latches `error` as a recording's refusal for an exchange the boundary
    /// could not even project (an unknown field, an unencodable number), so
    /// the exchange missing from the cassette leaves it without a file form
    /// exactly as a refused entry does. A replay is unchanged.
    pub fn refuse(&mut self, error: CassetteError) -> CassetteError {
        if self.redactor.is_some() {
            self.refused = Some(error.clone());
        }
        error
    }

    /// Strict lookup: a hit consumes the first unconsumed entry of `boundary`
    /// with an equal digest; a miss becomes the cassette's terminal.
    pub fn lookup(
        &mut self,
        namespace: &str,
        boundary: Boundary,
        request: &Value,
    ) -> Result<Lookup<'_>, CassetteError> {
        if self.redactor.is_some() {
            return Err(CassetteError::LookupOnRecord);
        }
        self.check_namespace(namespace)?;
        if let Some(terminal) = &self.terminal {
            return Ok(Lookup::Miss(terminal.clone()));
        }
        let digest = request_digest(request)?;
        let turn = self.lookups;
        self.lookups += 1;
        let candidates = || {
            (0..self.cases.len())
                .filter(|index| !self.consumed[*index] && self.cases[*index].boundary == boundary)
        };
        if let Some(index) = candidates().find(|index| self.cases[*index].request_digest == digest)
        {
            self.consumed[index] = true;
            return Ok(Lookup::Hit(&self.cases[index]));
        }
        let nearest = candidates().next().map(|index| &self.cases[index]).or(self
            .cases
            .iter()
            .rev()
            .find(|entry| entry.boundary == boundary));
        let class = match nearest {
            Some(entry) if tool_results_only(&entry.request, request) => MissClass::ToolResultDrift,
            _ => MissClass::ModelRequestChanged,
        };
        let miss = CassetteMiss {
            turn,
            class,
            request_digest: digest,
            nearest_recorded: nearest.map(|entry| entry.request_digest.clone()),
        };
        self.terminal = Some(miss.clone());
        Ok(Lookup::Miss(miss))
    }

    fn check_namespace(&self, offered: &str) -> Result<(), CassetteError> {
        if offered == self.namespace {
            Ok(())
        } else {
            Err(CassetteError::WrongNamespace {
                recorded: self.namespace.clone(),
                offered: offered.to_string(),
            })
        }
    }
}

/// One full-text scan, so no match can straddle a window edge; the scanner's
/// input cap is therefore the largest exchange a cassette admits.
fn admit(redactor: &Redactor, location: Location, value: &Value) -> Result<(), CassetteError> {
    let text = canonical_json_encode(value)?;
    let refused = |kind| CassetteError::RedactionRefused(location, kind);
    let redaction = redactor
        .redact(&text)
        .map_err(|error| refused(error.kind()))?;
    if redaction.detections.is_empty() {
        Ok(())
    } else {
        Err(refused(RedactionErrorKind::SecretDetected))
    }
}

pub fn request_digest(covered: &Value) -> Result<String, CassetteError> {
    Ok(protocol_digest(CASSETTE_REQUEST_DIGEST_PROTOCOL, covered)?)
}

impl OpenCodeRequest {
    /// Parses the raw body text; a body that is not a JSON object is refused
    /// rather than digested as empty.
    pub fn from_raw(
        path: &str,
        headers: BTreeMap<String, String>,
        body_text: &str,
    ) -> Result<Self, CassetteError> {
        let body: Value =
            serde_json::from_str(body_text).map_err(|_| CassetteError::MalformedBody)?;
        if !body.is_object() {
            return Err(CassetteError::MalformedBody);
        }
        Ok(Self {
            path: path.to_string(),
            headers,
            body,
        })
    }

    /// The covered projection: allowlisted headers, the covered body fields
    /// with volatile content removed, and the path. A body field outside the
    /// covered list is refused, so a provider that starts sending one fails
    /// loudly instead of replaying the wrong answer; fields the request omits
    /// are absent from the projection, so their absence is part of the digest.
    pub fn covered(&self) -> Result<Value, CassetteError> {
        let headers: Map<String, Value> = self
            .headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value))
            .filter(|(name, _)| OPENCODE_HEADER_ALLOWLIST.contains(&name.as_str()))
            .map(|(name, value)| (name, Value::String(value.clone())))
            .collect();
        let object = self.body.as_object().ok_or(CassetteError::MalformedBody)?;
        let mut body = Map::new();
        for (name, value) in object {
            if !OPENCODE_COVERED_FIELDS.contains(&format!("body.{name}").as_str()) {
                return Err(CassetteError::UnknownRequestField(name.clone()));
            }
            let value = match (name.as_str(), value.as_f64()) {
                ("temperature", Some(number)) => Value::String(canonical_decimal_f64(number)?),
                _ => strip_volatile(value, name == "system"),
            };
            body.insert(name.clone(), value);
        }
        Ok(serde_json::json!({
            "path": self.path,
            "headers": headers,
            "body": body,
        }))
    }
}

impl BackendRecord {
    pub fn covered(&self) -> Result<Value, CassetteError> {
        if let Some(temperature) = &self.temperature
            && !is_canonical_decimal(temperature)
        {
            return Err(CassetteError::TemperatureNotDecimal(temperature.clone()));
        }
        serde_json::to_value(self).map_err(|error| CassetteError::Shape(error.to_string()))
    }
}

/// Removes every `cache_control` member and, in the `system` blocks where the
/// provider's billing header lives (`billing`), normalizes the `cch=<nonce>;`
/// nonce in string values; the same text anywhere else is model-visible
/// content and stays as written.
fn strip_volatile(value: &Value, billing: bool) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(key, _)| key.as_str() != "cache_control")
                .map(|(key, member)| (key.clone(), strip_volatile(member, billing)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| strip_volatile(item, billing))
                .collect(),
        ),
        Value::String(text) if billing && text.contains("cch=") => {
            Value::String(normalize_nonce(text))
        }
        other => other.clone(),
    }
}

/// Rewrites each `cch=<nonce>;` whose nonce is a non-empty run of
/// alphanumerics, `_`, or `-` to `cch=<NONCE>;`; any other `cch=` text stays
/// as written.
fn normalize_nonce(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("cch=") {
        let after = &rest[start + 4..];
        let nonce_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .unwrap_or(after.len());
        if nonce_len > 0 && after[nonce_len..].starts_with(';') {
            out.push_str(&rest[..start]);
            out.push_str("cch=<NONCE>;");
            rest = &after[nonce_len + 1..];
        } else {
            out.push_str(&rest[..start + 4]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// `true` when the two covered projections differ only inside `tool_result`
/// block contents.
fn tool_results_only(recorded: &Value, offered: &Value) -> bool {
    recorded != offered && blank_tool_results(recorded) == blank_tool_results(offered)
}

fn blank_tool_results(value: &Value) -> Value {
    match value {
        Value::Object(members) => {
            if members.get("type").and_then(Value::as_str) == Some("tool_result") {
                let mut blank = members.clone();
                blank.remove("content");
                return Value::Object(blank);
            }
            Value::Object(
                members
                    .iter()
                    .map(|(key, member)| (key.clone(), blank_tool_results(member)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(blank_tool_results).collect()),
        other => other.clone(),
    }
}

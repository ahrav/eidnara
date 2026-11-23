//! Deterministic secret redaction backed by the in-tree scanner.

mod scanner;

use std::sync::LazyLock;

use secret_scanner::{
    ConstructionError, Finding, LimitExhausted, ScanError, ScanLimits, ScanProfile, Scanner,
};

pub const DETECTOR_ID: &str = "eidnara-secret-scanner";

/// Longest text `redact_durable_text` can inspect.
///
/// Above this the scan fails and the whole value is replaced by one placeholder,
/// because text that cannot be inspected cannot be shown to be secret-free. A
/// caller that must preserve its content has to reject the value at this length
/// instead of redacting it, since the placeholder is not recoverable.
pub const MAX_REDACTABLE_BYTES: usize = secret_scanner::MAX_INPUT_BYTES;

const LABEL_WORDS: &[&str] = &[
    "key",
    "token",
    "secret",
    "password",
    "auth",
    "authorization",
    "bearer",
    "credential",
];
const LABEL_QUALIFIERS: &[&str] = &[
    "api",
    "access",
    // A qualifier missing here is not a cosmetic gap: a compound name whose only label word
    // is the bare `key` reduces to a structural label, and a structural label skips content
    // scanning. `signing_key` must not be read the way `target_key` is.
    "signing",
    "signature",
    "webhook",
    "encryption",
    "hmac",
    "master",
    "refresh",
    "private",
    "client",
    "auth",
    "authorization",
    "secret",
    "bearer",
    "session",
    "service",
    "x",
    "openai",
    "anthropic",
    "google",
    "github",
    "huggingface",
    "aws",
    "azure",
    // The scanner's KEY_QUALIFIERS carries these and its contract test requires names like
    // `dbapikey`, so a qualifier it recognises and this gate does not is a bypass. Only the
    // credential prefixes are mirrored: the scanner also lists payload and encoding words
    // (`hash`, `value`, `id`, `path`, `data`, `file`, `env`) for matching *after* a secret
    // word, and treating those as credential qualifiers would make `file_key` and `data_key`
    // credentials, which is the false positive the bare-`key` carve-out exists to avoid.
    "db",
    "database",
    "admin",
    "root",
    "app",
    "application",
    "consumer",
    "oauth",
    "jwt",
    "shared",
    "user",
    "vault",
    "gcp",
    "gitlab",
    "slack",
    "stripe",
    "twilio",
];
/// Payload and encoding words the scanner matches after a secret word.
/// `apikeyvalue` reads as `api_key_value`.
/// Rejecting them as label segments keeps `file_key`, `value_key`, and `keyid` structural.
const LABEL_AFFIXES: &[&str] = &[
    "b64", "base64", "data", "env", "file", "hash", "hex", "id", "name", "path", "plain", "prefix",
    "ref", "string", "text", "value",
];
/// Segments that mark a key as public, mirroring `secret_scanner`'s `NON_SECRET_KEY_MARKERS`.
/// The scanner reports no keyed finding for these markers, so `key_names_a_secret` treats
/// `public_token` as structural. Otherwise the key gate refuses a value the scanner stores.
const NON_SECRET_KEY_MARKERS: &[&str] = &["public", "pubkey", "publishable"];
/// Longest word in the three vocabularies plus a plural suffix bounds cover-scan spans.
const MAX_VOCABULARY_WORD_BYTES: usize = "authorization".len() + 1;

/// One scanner finding in the original UTF-8 input.
///
/// `offset` and `length` are byte counts, not character counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub detector_id: &'static str,
    pub secret_type: String,
    pub offset: usize,
    pub length: usize,
}

/// Redacted text plus findings whose spans refer to the original input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redaction {
    pub text: String,
    pub detections: Vec<Detection>,
}

/// Stable classification for scanner construction, limit, span, and policy failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedactionErrorKind {
    Construction,
    InputLimit,
    CandidateLimit,
    WorkLimit,
    /// A candidate's full match exceeded `secret_scanner::MAX_MATCH_BYTES`, so the scan stopped instead of dropping it.
    MatchLimit,
    /// A windowed scan accumulated more detections than its caller allows.
    DetectionLimit,
    InvalidSpan,
    UnknownRule,
    /// A field that must stay verbatim was found to hold a secret, so the
    /// write is refused rather than redacted: redaction would alias
    /// distinct values onto one placeholder.
    SecretDetected,
}

/// Redaction failure that omits input and secret material from diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{}", redaction_error_message(.kind))]
pub struct RedactionError {
    pub(crate) kind: RedactionErrorKind,
}

impl RedactionError {
    #[must_use]
    pub const fn kind(self) -> RedactionErrorKind {
        self.kind
    }
}

fn redaction_error_message(kind: &RedactionErrorKind) -> &'static str {
    match kind {
        RedactionErrorKind::Construction => "secret redactor construction failed",
        RedactionErrorKind::InputLimit => "secret scan input limit exceeded",
        RedactionErrorKind::CandidateLimit => "secret scan candidate limit exceeded",
        RedactionErrorKind::WorkLimit => "secret scan work limit exceeded",
        RedactionErrorKind::MatchLimit => "secret scan match length limit exceeded",
        RedactionErrorKind::DetectionLimit => "secret scan detection limit exceeded",
        RedactionErrorKind::InvalidSpan => "secret scan produced an invalid span",
        RedactionErrorKind::UnknownRule => "secret scan produced an unclassified rule",
        RedactionErrorKind::SecretDetected => "secret-bearing field was rejected",
    }
}

impl From<ConstructionError> for RedactionError {
    fn from(_: ConstructionError) -> Self {
        Self {
            kind: RedactionErrorKind::Construction,
        }
    }
}

impl From<ScanError> for RedactionError {
    fn from(error: ScanError) -> Self {
        let kind = match error {
            ScanError::InputLimitExceeded => RedactionErrorKind::InputLimit,
            ScanError::InvalidSpan => RedactionErrorKind::InvalidSpan,
        };
        Self { kind }
    }
}

/// Produces deterministic redaction output.
pub struct Redactor {
    scanner: Scanner,
}

impl Redactor {
    /// Uses default scanner limits and reports construction failure without input data.
    pub fn new() -> Result<Self, RedactionError> {
        Self::with_limits(ScanLimits::default())
    }

    fn with_limits(limits: ScanLimits) -> Result<Self, RedactionError> {
        Ok(Self {
            scanner: Scanner::with_limits(ScanProfile::Comprehensive, limits)?,
        })
    }

    /// Returns no partial output when scanning exhausts a limit or yields an invalid span.
    pub fn redact(&self, input: &str) -> Result<Redaction, RedactionError> {
        let findings = WindowScan::new(self).findings(input, 0, true)?;
        scanner::render(input, scanner::describe_findings(input, &findings, 0)?)
    }

    /// Detections are counted after overlapping findings merge; exceeding `max_detections` returns before scanning later windows or rendering.
    pub fn redact_windowed(
        &self,
        input: &str,
        max_detections: usize,
    ) -> Result<Redaction, RedactionError> {
        let mut scan = WindowScan::new(self);
        let mut replacements = Vec::new();
        for (start, end) in scan_windows(input) {
            let window = window(input, start, end)?;
            let findings = scan.findings(window, start, end == input.len())?;
            scanner::merge_into(
                &mut replacements,
                scanner::describe_findings(window, &findings, start)?,
            );
            if replacements.len() > max_detections {
                return Err(RedactionError {
                    kind: RedactionErrorKind::DetectionLimit,
                });
            }
        }
        scanner::render(input, replacements)
    }

    /// Verdict-only counterpart of [`Self::redact_windowed`]: stops at the first finding and renders no output.
    pub fn detect_windowed(&self, input: &str) -> Result<bool, RedactionError> {
        let mut scan = WindowScan::new(self);
        for (start, end) in scan_windows(input) {
            if !scan
                .findings(window(input, start, end)?, start, end == input.len())?
                .is_empty()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Detects findings in bytes that need not be UTF-8.
    ///
    /// Valid UTF-8 uses [`Self::detect_windowed`] so byte and text scans share its window policy.
    /// Invalid UTF-8 is lossily decoded per window because U+FFFD expands to three UTF-8 bytes.
    pub fn detect_windowed_bytes(&self, bytes: &[u8]) -> Result<bool, RedactionError> {
        if let Ok(text) = std::str::from_utf8(bytes) {
            return self.detect_windowed(text);
        }
        self.detect_copying(&mut WindowScan::new(self), bytes)
    }

    /// Copies each window of the lossy decoding of `bytes` into a buffer and scans it.
    fn detect_copying(
        &self,
        scan: &mut WindowScan<'_>,
        bytes: &[u8],
    ) -> Result<bool, RedactionError> {
        let mut buffer = String::with_capacity(MAX_REDACTABLE_BYTES.min(bytes.len() + 3));
        // Byte offset of `buffer[0]` in the lossy text; zero marks the text's real left edge.
        let mut start = 0usize;
        for chunk in bytes.utf8_chunks() {
            let mut valid = chunk.valid();
            while !valid.is_empty() {
                let take = char_floor(valid, MAX_REDACTABLE_BYTES - buffer.len());
                if take == 0 {
                    // Sliding a full buffer frees at least one character because `WINDOW_OVERLAP_BYTES * 2 <= MAX_REDACTABLE_BYTES`.
                    debug_assert!(window_advance(&buffer) > 0);
                    if scan.slide(&mut buffer, &mut start)? {
                        return Ok(true);
                    }
                    continue;
                }
                buffer.push_str(&valid[..take]);
                valid = &valid[take..];
            }
            if !chunk.invalid().is_empty() {
                if buffer.len() + char::REPLACEMENT_CHARACTER.len_utf8() > MAX_REDACTABLE_BYTES
                    && scan.slide(&mut buffer, &mut start)?
                {
                    return Ok(true);
                }
                buffer.push(char::REPLACEMENT_CHARACTER);
            }
        }
        Ok(!scan.findings(&buffer, start, true)?.is_empty())
    }
}

/// One pass of overlapping windows over a text, in ascending order of `start`.
///
/// Each absolute byte position is claimed by exactly one window, so a finding
/// that two windows both see whole is reported once. Findings within
/// [`EDGE_MARGIN_BYTES`] of an artificial edge are left to the neighbouring window.
struct WindowScan<'a> {
    redactor: &'a Redactor,
    /// Absolute end of the range earlier windows have claimed.
    claimed_to: usize,
}

impl<'a> WindowScan<'a> {
    const fn new(redactor: &'a Redactor) -> Self {
        Self {
            redactor,
            claimed_to: 0,
        }
    }

    /// `start` is the window's byte offset in the full input; `is_last` indicates whether the window reaches its end.
    fn findings(
        &mut self,
        window: &str,
        start: usize,
        is_last: bool,
    ) -> Result<Vec<Finding>, RedactionError> {
        let mut report = self.redactor.scanner.scan(window)?;
        if let Some(limit) = report.limits_hit {
            return Err(RedactionError {
                kind: match limit {
                    LimitExhausted::Candidates => RedactionErrorKind::CandidateLimit,
                    LimitExhausted::Work => RedactionErrorKind::WorkLimit,
                    LimitExhausted::Match => RedactionErrorKind::MatchLimit,
                },
            });
        }
        let keep_from_rel = if start == 0 { 0 } else { EDGE_MARGIN_BYTES };
        let keep_to_rel = if is_last {
            window.len()
        } else {
            window.len().saturating_sub(EDGE_MARGIN_BYTES)
        };
        let claimed_to_rel = self.claimed_to.saturating_sub(start);
        report.findings.retain(|finding| {
            finding.full_span.start() >= keep_from_rel
                && finding.full_span.end() <= keep_to_rel
                && finding.full_span.end() > claimed_to_rel
        });
        self.claimed_to = start + keep_to_rel;
        Ok(report.findings)
    }

    /// Returns whether the scanned window held a finding.
    fn slide(&mut self, buffer: &mut String, start: &mut usize) -> Result<bool, RedactionError> {
        if !self.findings(buffer, *start, false)?.is_empty() {
            return Ok(true);
        }
        let cut = window_advance(buffer);
        buffer.drain(..cut);
        *start += cut;
        Ok(false)
    }
}

/// Advances by all but the trailing `WINDOW_OVERLAP_BYTES`, cut back to a char boundary.
fn window_advance(window: &str) -> usize {
    char_floor(window, window.len().saturating_sub(WINDOW_OVERLAP_BYTES))
}

fn window(input: &str, start: usize, end: usize) -> Result<&str, RedactionError> {
    input.get(start..end).ok_or(RedactionError {
        kind: RedactionErrorKind::InvalidSpan,
    })
}

/// The overlap contains a complete maximum-size finding footprint, so at least one window scans each finding intact.
///
/// A match longer than the overlap can span two windows without either window containing the match whole;
/// the windowed scan cannot see such a match. Only matches seen whole by one window are either reported
/// or fail the scan closed with `MatchLimit`.
pub const WINDOW_OVERLAP_BYTES: usize = 96 * 1024;

const _: () = assert!(
    secret_scanner::MAX_MATCH_BYTES
        + 2 * secret_scanner::MAX_RULE_RADIUS
        + 2 * secret_scanner::MAX_LOCAL_CONTEXT_BYTES
        <= WINDOW_OVERLAP_BYTES
);
const _: () = assert!(WINDOW_OVERLAP_BYTES * 2 <= MAX_REDACTABLE_BYTES);

/// Distance from an artificial window edge within which a finding is deferred to the neighbouring window.
///
/// At an artificial end-of-slice, `\b` and `$` may match and lookarounds may observe no adjacent input,
/// and a rule's radius is clipped. The margin is the most context any rule reads past its match on one side.
pub const EDGE_MARGIN_BYTES: usize =
    secret_scanner::MAX_RULE_RADIUS + secret_scanner::MAX_LOCAL_CONTEXT_BYTES;

// A deferred finding lies within `MAX_MATCH_BYTES + EDGE_MARGIN_BYTES` of the shared edge.
const _: () =
    assert!(secret_scanner::MAX_MATCH_BYTES + 2 * EDGE_MARGIN_BYTES <= WINDOW_OVERLAP_BYTES);
const _: () = assert!(EDGE_MARGIN_BYTES < MIN_WINDOW_ADVANCE_BYTES);

/// Yields `[start, end)` windows of at most `MAX_REDACTABLE_BYTES` whose starts fall on line boundaries and whose consecutive members share at least `WINDOW_OVERLAP_BYTES`.
/// A line longer than the window minus its overlap is split at a char boundary instead, so the walk always advances.
/// Input at or under `MAX_REDACTABLE_BYTES` is one window.
///
/// Each window is planned on demand, so a caller that stops at the first finding never pays for the rest.
fn scan_windows(input: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut next_start = Some(0usize);
    std::iter::from_fn(move || {
        let start = next_start.take()?;
        let end = char_floor(input, start.saturating_add(MAX_REDACTABLE_BYTES));
        if end < input.len() {
            let target = char_floor(input, end - WINDOW_OVERLAP_BYTES);
            // The latest line start at or before `target` keeps the overlap at least as wide as required.
            // Only line starts at or past `floor` qualify, preventing a long line preceded by a short one from shrinking the advance to a few bytes.
            // A newline before `floor - 1` cannot qualify, and searching from byte 0 each iteration is quadratic in newline-sparse input.
            let floor = start + MIN_WINDOW_ADVANCE_BYTES;
            let search_from = char_floor(input, floor.saturating_sub(1)).min(target);
            let next = input[search_from..target]
                .rfind('\n')
                .map(|index| search_from + index + 1)
                .filter(|&line_start| line_start >= floor)
                .unwrap_or(target);
            next_start = Some(next.max(start + 1));
        }
        Some((start, end))
    })
}

/// Smallest distance one window start moves past the previous one.
const MIN_WINDOW_ADVANCE_BYTES: usize = (MAX_REDACTABLE_BYTES - WINDOW_OVERLAP_BYTES) / 2;

/// Largest char boundary at or below `index`, clamped to the input length.
fn char_floor(input: &str, index: usize) -> usize {
    let mut index = index.min(input.len());
    while !input.is_char_boundary(index) {
        index -= 1;
    }
    index
}

static REDACTOR: LazyLock<Result<Redactor, RedactionError>> = LazyLock::new(Redactor::new);

/// Transaction bodies carry many fields at once, so they scan under higher
/// candidate and work ceilings than a single durable field. Without them a
/// large transaction would exhaust the default budget and be replaced whole.
const TRANSACTION_SCAN_LIMITS: ScanLimits = ScanLimits {
    max_input_bytes: secret_scanner::MAX_INPUT_BYTES,
    max_candidates: 524_288,
    max_work_bytes: 1024 * 1024 * 1024,
};

const _: () = {
    let default = ScanLimits::DEFAULT;
    assert!(TRANSACTION_SCAN_LIMITS.max_candidates >= default.max_candidates);
    assert!(TRANSACTION_SCAN_LIMITS.max_work_bytes >= default.max_work_bytes);
    assert!(TRANSACTION_SCAN_LIMITS.max_input_bytes >= default.max_input_bytes);
};

static TRANSACTION_REDACTOR: LazyLock<Result<Redactor, RedactionError>> =
    LazyLock::new(|| Redactor::with_limits(TRANSACTION_SCAN_LIMITS));

fn redactor() -> Result<&'static Redactor, RedactionError> {
    REDACTOR.as_ref().map_err(|error| *error)
}

fn transaction_redactor() -> Result<&'static Redactor, RedactionError> {
    TRANSACTION_REDACTOR.as_ref().map_err(|error| *error)
}

/// Replaces the whole field when scanner construction or scanning fails.
pub fn redact_durable_text(input: &str) -> Redaction {
    redact_with(redactor(), input)
}

fn redact_with(redactor: Result<&Redactor, RedactionError>, input: &str) -> Redaction {
    match redactor.and_then(|redactor| redactor.redact(input)) {
        Ok(redaction) => redaction,
        Err(_) => whole_placeholder(input),
    }
}

fn whole_placeholder(input: &str) -> Redaction {
    Redaction {
        text: "<REDACTED:secret>".to_owned(),
        detections: vec![Detection {
            detector_id: DETECTOR_ID,
            secret_type: "secret".to_owned(),
            offset: 0,
            length: input.len(),
        }],
    }
}

/// Redacts a transaction body under the transaction ceilings. Fails closed
/// the same way `redact_durable_text` does.
pub fn redact_transaction_durable_text(input: &str) -> Redaction {
    redact_with(transaction_redactor(), input)
}

/// Windowed redaction for content that is stored as itself, so no placeholder may stand in for the input on failure; callers must refuse the write on `Err`.
///
/// Input length is unbounded on this path; only the merged detection count is capped, at `max_detections`.
/// A match longer than [`WINDOW_OVERLAP_BYTES`] that straddles two windows is seen whole by neither and goes unreported.
pub fn redact_windowed_durable_text(
    input: &str,
    max_detections: usize,
) -> Result<Redaction, RedactionError> {
    redactor()?.redact_windowed(input, max_detections)
}

/// Windowed detection verdict; `Err` means the scan could not prove the input secret-free.
///
/// Input length is unbounded on this path; the walk stops at the first finding.
/// `Ok(false)` carries the [`WINDOW_OVERLAP_BYTES`] caveat: a match longer than the overlap that straddles two windows is not seen.
pub fn detect_windowed_durable_text(input: &str) -> Result<bool, RedactionError> {
    redactor()?.detect_windowed(input)
}

/// Windowed detection verdict over the lossy decoding of `bytes`; `Err` means the scan could not prove the input secret-free.
///
/// Input length is unbounded on this path; the walk stops at the first finding.
pub fn detect_windowed_durable_bytes(bytes: &[u8]) -> Result<bool, RedactionError> {
    redactor()?.detect_windowed_bytes(bytes)
}

/// Identifies the detector build that produced a redaction, for the audit
/// receipt a durable write records alongside it.
pub fn detector_revision() -> String {
    redactor().map_or_else(
        |_| "unavailable".to_owned(),
        |redactor| {
            let revision = redactor.scanner.revision();
            format!(
                "{}:{}:{}",
                revision.crate_version, revision.semantic_digest_version, revision.upstream_commit
            )
        },
    )
}

/// Digest of the rule semantics behind `redact_durable_text`, the windowed
/// paths, and `reject_secret_text`, or `None` when the detector could not be
/// built.
///
/// The digest covers the scan limits, so a receipt for a transaction-path
/// redaction records [`transaction_detector_semantic_digest`] instead.
pub fn detector_semantic_digest() -> Option<[u8; 32]> {
    semantic_digest_of(redactor())
}

/// Digest of the rule semantics behind `redact_transaction_durable_text` and
/// `reject_transaction_secret_text`, or `None` when the detector could not be
/// built.
pub fn transaction_detector_semantic_digest() -> Option<[u8; 32]> {
    semantic_digest_of(transaction_redactor())
}

fn semantic_digest_of(redactor: Result<&Redactor, RedactionError>) -> Option<[u8; 32]> {
    redactor
        .ok()
        .map(|redactor| redactor.scanner.semantic_digest())
}

/// Refuses `input` when the scanner classifies any of it as secret. Used
/// for fields that must stay verbatim, such as lookup keys and dedup
/// identities, where substituting a placeholder would merge distinct
/// values.
pub fn reject_secret_text(input: &str) -> Result<(), RedactionError> {
    reject(redact_durable_text(input))
}

/// Transaction-ceiling counterpart of `reject_secret_text`.
pub fn reject_transaction_secret_text(input: &str) -> Result<(), RedactionError> {
    reject(redact_transaction_durable_text(input))
}

fn reject(redaction: Redaction) -> Result<(), RedactionError> {
    if redaction.detections.is_empty() {
        Ok(())
    } else {
        Err(RedactionError {
            kind: RedactionErrorKind::SecretDetected,
        })
    }
}

/// Label for a key whose name alone marks its value secret, so a value
/// written under it is replaced even when the scanner finds nothing in it.
/// `None` when the name carries no secret word or carries one of
/// `NON_SECRET_KEY_MARKERS`, as in `public_token`.
pub fn secret_key_label(key: &str) -> Option<String> {
    key_names_a_secret(key).then(|| redaction_type_for_key(key))
}

/// `key` and `keys` name JSON map entries rather than secrets. They are excluded
/// case-insensitively; `api_key` remains protected.
///
/// Recognition matches [`secret_shaped_json_key`], so a separator-free compound such as
/// `apikey` is protected too. A narrower test leaves that value neither substituted nor
/// refused, because the leaf scanner sees only the value and gains no key context from it.
pub fn protected_json_key_label(key: &str) -> Option<String> {
    secret_shaped_json_key(key).then(|| redaction_type_for_key(key))
}

/// Returns a label only when `key` has a credential qualifier.
///
/// `api_key` and `private_key` carry one.
/// `target_key` and `model_key` reduce to the bare `key` label and name structural rows.
pub fn qualified_secret_key_label(key: &str) -> Option<String> {
    secret_key_label(key).filter(|label| !is_bare_key_label(label))
}

/// Whether a redaction label is the bare `key`/`keys`, which names a JSON map entry rather than a credential.
fn is_bare_key_label(label: &str) -> bool {
    label.eq_ignore_ascii_case("key") || label.eq_ignore_ascii_case("keys")
}

/// Whether a JSON field name has the shape the keyed scanner rules anchor on.
///
/// Keyed rules accept compound keys, unlike [`secret_key_label`].
/// Matching covers segments and the separator-free spelling of the same name.
/// Whole label words only: a substring test reads `author` as `auth`.
/// A substring test also misses `passWord`, whose split `pass_word` holds no label word.
///
/// `key` and `keys` are excluded case-insensitively, as in [`protected_json_key_label`].
pub fn secret_shaped_json_key(key: &str) -> bool {
    if is_bare_key_label(key) {
        return false;
    }
    if key_names_a_secret(key) {
        // A name whose label reduces to the bare `key` carries no credential qualifier, so
        // it names a structural row: `target_key` and `stream_key`, not `api_key`. Reading
        // it as a credential refuses ordinary vocabulary, and disagreeing with
        // `qualified_secret_key_label` would make one name structural to the identity gate
        // and secret to this one.
        return !is_bare_key_label(&redaction_type_for_key(key));
    }
    let joined = separate_words(key)
        .to_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>();
    undelimited_names_a_credential(&joined)
}

/// A full vocabulary cover must hold a label word and a non-`key` label segment.
///
/// A qualifier can precede the label and an affix can follow it.
/// [`vocabulary_reach`] searches both ends so `apikeyvalue` is classified like `api_key_value`.
/// A prefix-only walk classifies `apikeyvalue` as structural.
///
/// Affixes cover a name without qualifying it, so `keyvalue` and `keyid` stay structural
/// exactly as `key_value` and `key_id` do.
fn undelimited_names_a_credential(joined: &str) -> bool {
    if !LABEL_WORDS.iter().any(|word| joined.contains(word)) {
        return false;
    }
    let from_start = vocabulary_reach(joined, false);
    if !from_start[joined.len()] {
        return false;
    }
    let to_end = vocabulary_reach(joined, true);
    let mut names_a_label = false;
    let mut names_a_qualified_segment = false;
    for start in 0..joined.len() {
        if !from_start[start] {
            continue;
        }
        let last = (start + MAX_VOCABULARY_WORD_BYTES).min(joined.len());
        for end in start + 1..=last {
            if !to_end[end] {
                continue;
            }
            let word = &joined[start..end];
            if !names_a_vocabulary_word(word) {
                continue;
            }
            names_a_label |= names_a_label_word(word);
            names_a_qualified_segment |= is_label_segment(word) && plural_stem(word) != "key";
        }
    }
    names_a_label && names_a_qualified_segment
}

/// Returns positions reachable by a vocabulary cover from either end of `joined`.
/// A position both walks reach is a word boundary of some cover.
fn vocabulary_reach(joined: &str, reverse: bool) -> Vec<bool> {
    let length = joined.len();
    let mut reach = vec![false; length + 1];
    reach[if reverse { length } else { 0 }] = true;
    for step in 0..=length {
        let at = if reverse { length - step } else { step };
        if !reach[at] {
            continue;
        }
        for span in 1..=MAX_VOCABULARY_WORD_BYTES {
            let (start, end) = if reverse {
                match at.checked_sub(span) {
                    Some(start) => (start, at),
                    None => break,
                }
            } else if at + span <= length {
                (at, at + span)
            } else {
                break;
            };
            if names_a_vocabulary_word(&joined[start..end]) {
                reach[if reverse { start } else { end }] = true;
            }
        }
    }
    reach
}

/// Stemming `aws` yields `aw`, which no vocabulary holds, so a qualifier matches whole.
fn names_a_vocabulary_word(word: &str) -> bool {
    let stem = plural_stem(word);
    LABEL_WORDS.contains(&stem)
        || LABEL_QUALIFIERS.contains(&word)
        || LABEL_AFFIXES.contains(&word)
        || LABEL_AFFIXES.contains(&stem)
}

/// Whether `word` is a label word, tolerating a plural suffix as the segment test does.
fn names_a_label_word(word: &str) -> bool {
    LABEL_WORDS.contains(&plural_stem(word))
}

fn plural_stem(word: &str) -> &str {
    word.strip_suffix('s').unwrap_or(word)
}

fn key_names_a_secret(key: &str) -> bool {
    let lowered = separate_words(key).to_lowercase();
    let mut segments = lowered.split(|character: char| !character.is_ascii_alphanumeric());
    if segments
        .clone()
        .any(|segment| NON_SECRET_KEY_MARKERS.contains(&segment))
    {
        return false;
    }
    segments.any(|segment| {
        let stem = segment.strip_suffix('s').unwrap_or(segment);
        LABEL_WORDS.contains(&stem) || ends_with_password(segment)
    })
}

/// A segment that ends in `password` names one whatever precedes it (`pgpassword`), as the
/// scanner's key gate reads it. The other label words end ordinary words (`monkey`).
fn ends_with_password(segment: &str) -> bool {
    segment.ends_with("password")
}

/// Substrings that identify the `<LABEL_REDACTED>` and `<REDACTED:label>` marker shapes.
pub const REDACTION_TOKEN_MARKERS: &[&str] = &["_REDACTED>", "<REDACTED:"];

/// Recognizes both supported redaction marker shapes without validating labels.
#[must_use]
pub fn contains_redaction_token(text: &str) -> bool {
    REDACTION_TOKEN_MARKERS
        .iter()
        .any(|marker| text.contains(marker))
}

/// Maximum UTF-8 byte length of a key-derived redaction label.
pub const MAX_REDACTION_LABEL_BYTES: usize = 64;

fn redaction_type_for_key(key: &str) -> String {
    // A label holds vocabulary words only. A segment that is a password by its ending alone
    // contributes the word `password`, so text of the key, which another finding can cover,
    // stays out of the label.
    let label = separate_words(key)
        .to_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter_map(|segment| {
            if is_label_segment(segment) {
                Some(segment)
            } else {
                ends_with_password(segment).then_some("password")
            }
        })
        .collect::<Vec<_>>()
        .join("_");
    if label.is_empty() {
        return "secret".to_owned();
    }
    if label.len() <= MAX_REDACTION_LABEL_BYTES {
        return label;
    }
    // Truncate on a segment boundary so the label stays a `_`-joined run of whole label
    // words. Cutting mid-segment would invent a word that no key contained.
    match label[..=MAX_REDACTION_LABEL_BYTES].rfind('_') {
        Some(boundary) if boundary > 0 => label[..boundary].to_owned(),
        _ => "secret".to_owned(),
    }
}

/// Splits camel case so `apiKey` yields the same segments as `api_key`.
fn separate_words(key: &str) -> String {
    let characters = key.chars().collect::<Vec<_>>();
    let mut separated = String::with_capacity(key.len() + 8);
    for (index, character) in characters.iter().copied().enumerate() {
        let previous = index.checked_sub(1).map(|earlier| characters[earlier]);
        let lower_to_upper = character.is_ascii_uppercase()
            && previous
                .is_some_and(|earlier| earlier.is_ascii_lowercase() || earlier.is_ascii_digit());
        // `URLToken` ends an acronym at the last capital of the run, so the boundary sits
        // before a capital whose successor is lowercase. Without it the key stays one
        // segment, `urltoken`, and no label word matches.
        let acronym_to_word = character.is_ascii_uppercase()
            && previous.is_some_and(|earlier| earlier.is_ascii_uppercase())
            && characters
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_lowercase());
        if lower_to_upper || acronym_to_word {
            separated.push('_');
        }
        separated.push(character);
    }
    separated
}

fn is_label_segment(segment: &str) -> bool {
    let stem = segment.strip_suffix('s').unwrap_or(segment);
    LABEL_WORDS.contains(&stem) || LABEL_QUALIFIERS.contains(&segment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_windows(input: &str) -> Vec<(usize, usize)> {
        let windows = scan_windows(input).collect::<Vec<_>>();
        assert_eq!(windows[0].0, 0);
        assert_eq!(windows.last().unwrap().1, input.len());
        for pair in windows.windows(2) {
            let (first, second) = (pair[0], pair[1]);
            assert!(first.1 - first.0 <= MAX_REDACTABLE_BYTES);
            assert!(second.0 > first.0, "window start regressed: {pair:?}");
            assert!(
                first.1 - second.0 >= WINDOW_OVERLAP_BYTES,
                "overlap too small: {pair:?}"
            );
            assert!(
                second.0 - first.0 >= MIN_WINDOW_ADVANCE_BYTES,
                "advance too small: {pair:?}"
            );
            assert!(input.is_char_boundary(second.0));
        }
        windows
    }

    #[test]
    fn windows_start_on_line_boundaries_and_overlap_when_lines_are_short() {
        let input = "0123456789abcdef\n".repeat((3 * MAX_REDACTABLE_BYTES) / 17);
        let windows = check_windows(&input);
        assert!(windows.len() >= 3);
        for &(start, _) in &windows[1..] {
            assert_eq!(&input[start - 1..start], "\n");
        }
    }

    #[test]
    fn a_single_long_line_still_advances_by_the_minimum() {
        let mut input = "short\n".to_owned();
        input.push_str(&"y".repeat(2 * MAX_REDACTABLE_BYTES));
        let windows = check_windows(&input);
        assert!(windows.len() <= 2 * MAX_REDACTABLE_BYTES / MIN_WINDOW_ADVANCE_BYTES + 2);
    }

    #[test]
    fn windows_never_split_a_multibyte_char() {
        check_windows(&"π".repeat(MAX_REDACTABLE_BYTES));
        check_windows(&"\u{fffd}".repeat(MAX_REDACTABLE_BYTES));
    }

    #[test]
    fn short_input_uses_a_single_window() {
        assert_eq!(scan_windows("a\nb").collect::<Vec<_>>(), [(0, 3)]);
        assert_eq!(scan_windows("").collect::<Vec<_>>(), [(0, 0)]);
    }

    #[test]
    fn scanner_is_the_only_redaction_path() {
        let redaction = redact_durable_text("π password=hunter-two");
        assert_eq!(redaction.text, "π password=<REDACTED:password>");
        assert!(
            redaction
                .detections
                .iter()
                .all(|detection| detection.detector_id == DETECTOR_ID)
        );
    }

    /// The detector id is written into durable evidence rows, so its literal
    /// value is pinned here rather than compared with itself.
    #[test]
    fn detector_id_is_the_recorded_literal() {
        assert_eq!(DETECTOR_ID, "eidnara-secret-scanner");
    }

    #[test]
    fn each_redaction_path_reports_its_own_semantic_digest() {
        let default = detector_semantic_digest().unwrap();
        let transaction = transaction_detector_semantic_digest().unwrap();
        assert_ne!(default, transaction);
        assert_eq!(default, Redactor::new().unwrap().scanner.semantic_digest());
        assert_eq!(
            transaction,
            Redactor::with_limits(TRANSACTION_SCAN_LIMITS)
                .unwrap()
                .scanner
                .semantic_digest()
        );
    }

    #[test]
    fn established_key_labels_remain_stable() {
        for (key, expected) in [
            ("x_auth_token", "x_auth_token"),
            ("client_secret", "client_secret"),
            ("aws_secret", "aws_secret"),
            ("apiKey", "api_key"),
            ("apikey", "secret"),
        ] {
            assert_eq!(redaction_type_for_key(key), expected, "{key}");
        }
    }

    #[test]
    fn scanner_limits_fail_closed() {
        let redactor = Redactor::with_limits(ScanLimits {
            max_work_bytes: 1,
            ..ScanLimits::default()
        })
        .unwrap();
        assert_eq!(
            redactor.redact("password=secret").unwrap_err().kind(),
            RedactionErrorKind::WorkLimit
        );
    }

    #[test]
    fn diagnostics_never_include_input() {
        let sentinel = "privacy-sentinel-password=secret";
        let input = format!("{sentinel}{}", "x".repeat(secret_scanner::MAX_INPUT_BYTES));
        let error = Redactor::new()
            .unwrap()
            .redact(&input)
            .unwrap_err()
            .to_string();
        assert!(!error.contains(sentinel));
        assert!(!error.contains("secret="));
    }

    #[test]
    fn secret_shaped_keys_match_whole_label_words() {
        for key in [
            "passWord",
            "apikey",
            "authtoken",
            "api_key",
            "client_secret",
            "Authorization",
            "bearerToken",
        ] {
            assert!(
                secret_shaped_json_key(key),
                "expected {key} to be protected"
            );
        }
        for key in [
            "author",
            "authored_by",
            "key",
            "keys",
            "monkey",
            "keyboard",
            "display_path",
        ] {
            assert!(
                !secret_shaped_json_key(key),
                "expected {key} to stay writable"
            );
        }
    }

    #[test]
    fn only_qualified_key_names_mark_a_credential() {
        for key in [
            "api_key",
            "private_key",
            "access_key",
            "apiKey",
            "aws_secret_access_key",
        ] {
            assert!(
                qualified_secret_key_label(key).is_some(),
                "expected {key} to name a credential"
            );
        }
        for key in [
            "key",
            "keys",
            "last_model_key",
            "lineage_descent_target_key",
            "lineage_descent_source_key",
        ] {
            assert!(
                qualified_secret_key_label(key).is_none(),
                "expected {key} to name a structural row"
            );
        }
    }

    #[test]
    fn an_acronym_boundary_still_separates_label_words() {
        for key in ["URLToken", "SQLSecret", "APISecret", "AWSSecretKey"] {
            assert!(
                secret_shaped_json_key(key),
                "expected {key} to be protected"
            );
        }
    }
}

#[cfg(test)]
mod qualifier_chain_tests {
    use super::*;

    /// The bare-`key` carve-out must only exempt names with no credential qualifier. A
    /// qualifier absent from `LABEL_QUALIFIERS` makes a credential name reduce to the
    /// structural label `key`, and a structural label skips content scanning entirely.
    /// Both gates must agree on every name.
    #[test]
    fn a_credential_qualifier_keeps_a_key_name_scanned() {
        for key in [
            "api_key",
            "session_key",
            "private_key",
            "client_secret",
            "signing_key",
            "signingKey",
            "webhook_key",
            "encryption_key",
            "hmac_key",
            "master_key",
            "refresh_token",
            "signature_key",
        ] {
            assert!(
                secret_shaped_json_key(key),
                "{key} carries a credential qualifier and must be scanned"
            );
            assert!(
                qualified_secret_key_label(key).is_some(),
                "{key} must resolve to a qualified label"
            );
        }

        // The carve-out itself must survive: these carry no credential qualifier.
        for key in [
            "target_key",
            "stream_key",
            "primary_key",
            "key_id",
            "last_model_key",
            "key",
        ] {
            assert!(
                !secret_shaped_json_key(key),
                "{key} names a structural row and must stay writable"
            );
            assert!(qualified_secret_key_label(key).is_none(), "{key}");
        }
    }

    /// An undelimited name can chain several qualifiers before its label word, so stripping
    /// only the first leaves a remainder that matches nothing.
    #[test]
    fn chained_qualifiers_still_reach_the_label_word() {
        for key in ["awssecretaccesskey", "awsaccesskey", "clientsecretkey"] {
            assert!(secret_shaped_json_key(key), "{key} must be scanned");
        }
    }

    /// This gate keeps its own qualifier vocabulary, so it can silently fall behind the
    /// scanner's. These names are the ones `secret-scanner`'s own contract test requires
    /// the scanner to flag; a name the scanner treats as a credential must not be structural
    /// here, because structural means the value skips content scanning entirely.
    #[test]
    fn the_key_gate_agrees_with_the_scanner_contract_vocabulary() {
        for key in [
            "dbapikey",
            "databasepassword",
            "adminpassword",
            "rootpassword",
            "oauthtoken",
            "jwtsecret",
            "sharedsecret",
            "usertoken",
            "mastertoken",
            "servicekey",
            "vaultsecret",
            "slacktoken",
            "APIKEYVALUE",
            "tokenvalue",
            "secretname",
            "passwordhash",
        ] {
            assert!(
                secret_shaped_json_key(key),
                "the scanner flags {key}; this gate must not treat it as structural"
            );
        }

        // A payload word does not qualify a label word when it precedes or follows it.
        for key in [
            "file_key",
            "data_key",
            "env_key",
            "path_key",
            "value_key",
            "keyvalue",
            "keyid",
            "keyfile",
        ] {
            assert!(
                !secret_shaped_json_key(key),
                "{key} carries no credential qualifier and must stay writable"
            );
        }
    }

    /// Undelimited names follow delimiter-separated field-name semantics.
    /// A trailing qualifier is covered as well as a leading one.
    #[test]
    fn an_undelimited_name_is_gated_like_its_delimited_spelling() {
        for (delimited, undelimited) in [
            ("api_key_value", "apikeyvalue"),
            ("api_key_value", "APIKEYVALUE"),
            ("token_value", "tokenvalue"),
            ("secret_name", "secretname"),
            ("password_hash", "passwordhash"),
            ("aws_secret_access_key", "awssecretaccesskey"),
            ("api_keys", "apikeys"),
            ("key_value", "keyvalue"),
            ("key_id", "keyid"),
            ("key_file", "keyfile"),
            ("value_key", "valuekey"),
            ("file_key", "filekey"),
            ("data_key", "datakey"),
            ("public_key", "publickey"),
            ("public_token", "publictoken"),
            ("publishable_key", "publishablekey"),
        ] {
            assert_eq!(
                secret_shaped_json_key(delimited),
                secret_shaped_json_key(undelimited),
                "{delimited} and {undelimited} name the same field"
            );
        }
    }

    /// The scanner's `NON_SECRET_KEY_MARKERS` exclude a key from keyed findings, so the key
    /// gates must not read it as a credential either.
    #[test]
    fn public_marked_keys_are_structural_in_every_gate() {
        for key in [
            "public_token",
            "publicToken",
            "PUBLIC_SECRET",
            "publishable_key",
            "stripe_publishable_key",
            "pubkey_token",
        ] {
            assert!(!secret_shaped_json_key(key), "{key}");
            assert_eq!(secret_key_label(key), None, "{key}");
            assert_eq!(qualified_secret_key_label(key), None, "{key}");
            assert_eq!(protected_json_key_label(key), None, "{key}");
        }
        for key in ["api_token", "client_secret", "publicity_token"] {
            assert!(secret_shaped_json_key(key), "{key}");
            assert!(secret_key_label(key).is_some(), "{key}");
        }
    }

    /// The cover scan stops at [`MAX_VOCABULARY_WORD_BYTES`].
    #[test]
    fn the_vocabulary_stays_within_the_scan_bound() {
        for word in LABEL_WORDS
            .iter()
            .chain(LABEL_QUALIFIERS)
            .chain(LABEL_AFFIXES)
        {
            assert!(
                word.len() < MAX_VOCABULARY_WORD_BYTES,
                "{word} plus a plural exceeds the cover-scan span"
            );
        }
    }
}

#[cfg(test)]
mod credential_form_tests {
    use super::*;

    /// Credential values are joined at run time, so each line of this file stays clear of the
    /// shapes the repository's secret scanners reject.
    fn issued() -> String {
        ["Zq7Lm2Xv", "9Rt4Kp8W", "n3Bc6Yd1"].concat()
    }

    /// A short password a person would choose.
    fn chosen() -> String {
        ["hunter", "2"].concat()
    }

    fn url(scheme: &str, user: &str, password: &str, rest: &str) -> String {
        format!("{scheme}://{user}:{password}@{rest}")
    }

    /// The Base64 of `alice:` and the chosen password, as an `Authorization: Basic` header
    /// carries it.
    fn basic_credentials() -> String {
        ["YWxpY2U6", "aHVudGVyMg=="].concat()
    }

    fn private_key_header() -> String {
        format!("-----BEGIN {} KEY-----", ["RSA ", "PRIVATE"].concat())
    }

    /// One line of a private-key body: 64 Base64 characters.
    fn private_key_body_line() -> String {
        let line = [
            "QUJDREVGR0hJSktM",
            "TU5PUFFSU1RVVldY",
            "WVphYmNkZWZnaGlq",
            "a2xtbm9wcXJzdHV2",
        ]
        .concat();
        assert_eq!(line.len(), 64);
        line
    }

    /// The value is replaced, everything else is kept, and a caller that refuses on a
    /// finding refuses this text.
    #[track_caller]
    fn assert_redacts(input: &str, value: &str, placeholder: &str) {
        assert_eq!(
            input.matches(value).count(),
            1,
            "{input:?} holds {value:?} once"
        );
        let redaction = redact_durable_text(input);
        assert_eq!(
            redaction.text,
            input.replacen(value, placeholder, 1),
            "{input:?}"
        );
        assert_eq!(redaction.detections.len(), 1, "{input:?}");
        let detection = &redaction.detections[0];
        assert_eq!(detection.offset, input.find(value).unwrap(), "{input:?}");
        assert_eq!(detection.length, value.len(), "{input:?}");
        assert_eq!(
            format!("<REDACTED:{}>", detection.secret_type),
            placeholder,
            "{input:?}"
        );
        assert_eq!(
            reject_secret_text(input).unwrap_err().kind(),
            RedactionErrorKind::SecretDetected,
            "{input:?}"
        );
    }

    /// The text is free of findings, so a substituting caller stores it as written and a
    /// refusing caller accepts it.
    #[track_caller]
    fn assert_exact(input: &str) {
        let redaction = redact_durable_text(input);
        assert_eq!(redaction.text, input);
        assert_eq!(redaction.detections, [], "{input:?}");
        assert_eq!(reject_secret_text(input), Ok(()), "{input:?}");
    }

    #[test]
    fn an_option_followed_by_its_value_is_redacted() {
        let (issued, chosen) = (issued(), chosen());
        for (input, value, placeholder) in [
            (
                format!("deploy --password {chosen} --verbose"),
                chosen.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("tool --api-key {issued} run"),
                issued.clone(),
                "<REDACTED:api_key>",
            ),
            (
                format!("tool -token {issued}"),
                issued.clone(),
                "<REDACTED:token>",
            ),
            (
                format!("psql --db-password\t{chosen}"),
                chosen.clone(),
                "<REDACTED:db_password>",
            ),
            (
                format!("cli --authToken {issued} --json"),
                issued.clone(),
                "<REDACTED:auth_token>",
            ),
            (
                format!("cli --client-secret '{issued}' --json"),
                issued.clone(),
                "<REDACTED:client_secret>",
            ),
            (
                format!("deploy --password \"{chosen} and more\" --verbose"),
                format!("{chosen} and more"),
                "<REDACTED:password>",
            ),
        ] {
            assert_redacts(&input, &value, placeholder);
        }
    }

    /// Help text, prose, and scripts put a stand-in where the credential would go.
    #[test]
    fn an_option_followed_by_a_stand_in_stays_exact() {
        for input in [
            "  -p, --password string   Password for the registry",
            "usage: tool [--password PASSWORD] [--token TOKEN]",
            "tool login --password <password>",
            "pass the --password flag; the --token option is required.",
            "deploy --password \"$DB_PASSWORD\" --token '${TOKEN}'",
            "deploy --password -u root7",
            "deploy --password\nnext7 line",
            "run --max-tokens 4096 --output-tokens 80x",
            "run --token-file ./token7.txt --key ./server7.key",
            "collect --secret ~/secrets/share-7.txt --secret /path/to/secrets.txt",
            "docker build --secret id=npm,src=.npmrc7 .",
            "litellm --api-key os.environ/OPENAI_API_KEY",
            "gh auth status --token gho_************************************",
            "see your-password 4now",
            "call --token \\n  next7",
        ] {
            assert_exact(input);
        }
    }

    /// `-p` is a password only under a program that defines it so.
    #[test]
    fn a_password_option_of_a_named_client_is_redacted() {
        let (issued, chosen) = (issued(), chosen());
        for (input, value) in [
            (format!("mysql -uroot -p{chosen} app"), chosen.clone()),
            (
                format!("mysqldump -h db -P 3306 -u root -p{issued} app > dump.sql"),
                issued.clone(),
            ),
            (
                format!("mariadb -u root -p'{chosen} two' -h db"),
                format!("{chosen} two"),
            ),
            (
                format!("sshpass -p {chosen} ssh deploy@host"),
                chosen.clone(),
            ),
            (
                format!("sshpass -p \"{issued}\" ssh deploy@host"),
                issued.clone(),
            ),
            (
                format!("docker login -u bob -p {issued} registry.internal"),
                issued.clone(),
            ),
            (
                format!("helm registry login registry.internal -u bob -p {issued}"),
                issued.clone(),
            ),
        ] {
            assert_redacts(&input, &value, "<REDACTED:password>");
        }
    }

    #[test]
    fn a_short_option_of_another_program_stays_exact() {
        for input in [
            "ssh -l login -p 2222 host",
            "docker run -u 1000:1000 -p 8080:80 image",
            "mkdir -p a1/b2 && cargo test -p crate-9",
            "pytest tests/login -p no:cacheprovider",
            "mysql -u root -p",
            "mysql -h db -P 3306 app",
            "mysql app < dump.sql | patch -p1x",
            "mysql -pPASSWORD app",
            "docker login -u bob -p $(cat pw7.txt) registry",
            "docker login -u bob -p \"$REGISTRY_PASSWORD\" registry",
            "docker ps; tool login -p abc123",
        ] {
            assert_exact(input);
        }
    }

    #[test]
    fn a_password_in_url_userinfo_is_redacted() {
        let (issued, chosen) = (issued(), chosen());
        for (input, value) in [
            (
                format!(
                    "git clone {}",
                    url("https", "alice", &chosen, "example.org/r.git")
                ),
                chosen.clone(),
            ),
            (
                url(
                    "postgres",
                    "app",
                    &issued,
                    "db.internal:5432/app?sslmode=require",
                ),
                issued.clone(),
            ),
            (url("redis", "", &chosen, "cache:6379/0"), chosen.clone()),
            (
                format!(
                    "DATABASE_URL=\"{}\"",
                    url("mysql", "root", &issued, "127.0.0.1/app")
                ),
                issued.clone(),
            ),
            (
                url("https", "bob", &format!("p@ss:{chosen}"), "host/"),
                format!("p@ss:{chosen}"),
            ),
        ] {
            assert_redacts(&input, &value, "<REDACTED:url_password>");
        }
    }

    #[test]
    fn a_url_with_a_stand_in_or_a_bare_user_stays_exact() {
        for input in [
            url("postgres", "user", "password", "localhost/db"),
            url("https", "u", "$PASS", "host/"),
            url("https", "u", "{password}", "host/"),
            url("https", "u", "%s", "%s"),
            url("https", "u", "****", "host/"),
            "ssh://git@host:22/repo.git".to_owned(),
            "https://user@host/path".to_owned(),
            "http://localhost:8080/x?mail=a@b7.example".to_owned(),
            "http://[::1]:8080/a@b7".to_owned(),
            "see http://example.org: mail a7@example.org".to_owned(),
        ] {
            assert_exact(&input);
        }
    }

    #[test]
    fn basic_and_bearer_authorization_headers_are_redacted() {
        let issued = issued();
        let basic = basic_credentials();
        for (input, value, placeholder) in [
            (
                format!("Authorization: Basic {basic}"),
                basic.clone(),
                "<REDACTED:basic>",
            ),
            (
                format!("curl -H 'Proxy-Authorization: Basic {basic}' https://h/x"),
                basic.clone(),
                "<REDACTED:basic>",
            ),
            (
                format!("Authorization: Bearer {issued}"),
                issued.clone(),
                "<REDACTED:bearer>",
            ),
        ] {
            assert_redacts(&input, &value, placeholder);
        }
        // The scheme name followed by prose decodes to bytes other than `user:password` text.
        assert_exact("Authorization: Basic authentication is what the proxy expects");
        assert_exact("Authorization: Basic dXNlcg==");
    }

    #[test]
    fn a_field_line_is_redacted() {
        let (issued, chosen) = (issued(), chosen());
        for (input, value, placeholder) in [
            (
                format!("password: {chosen}"),
                chosen.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("    12\u{2192}  password: {chosen}  # rotate me\nnext: line"),
                chosen.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("user: bob\r\nPassword: {chosen}\r\n"),
                chosen.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("< X-Api-Key: {issued}"),
                issued.clone(),
                "<REDACTED:x_api_key>",
            ),
            (
                format!("http POST https://h/x \"X-Api-Key: {chosen}\""),
                chosen.clone(),
                "<REDACTED:x_api_key>",
            ),
            (
                format!("curl -H 'X-Auth-Token: {issued}' https://h/x"),
                issued.clone(),
                "<REDACTED:x_auth_token>",
            ),
            (
                format!("  db_password: \"{chosen}\","),
                chosen.clone(),
                "<REDACTED:db_password>",
            ),
            (
                format!("const client = new Client({{ apiKey: '{issued}', retries: 2 }})"),
                issued.clone(),
                "<REDACTED:api_key>",
            ),
            (
                format!("access_token: {issued}|{chosen}&x"),
                format!("{issued}|{chosen}&x"),
                "<REDACTED:access_token>",
            ),
        ] {
            assert_redacts(&input, &value, placeholder);
        }
    }

    /// Source code and reports write `name: value` for types, counts, and references.
    #[test]
    fn a_field_of_types_counts_and_references_stays_exact() {
        for input in [
            "interface A { password: string; token: string }",
            "  token: u64,\n  secret: Optional[str]\n  max_tokens: 5\n  api_key: str | None",
            "  token: 92,\n  password: undefined,",
            "Token: Temp/x9 words (\"for\", \"since\")",
            "Tokens: run900/data/tokens.parquet",
            "      api_key: os.environ/ZHIPU_API_KEY",
            "  - Token: gho_************************************",
            "  password: \"Password must be at least 8 characters\"",
            "  apiKey: \"$LOCAL_OPENAI_API_KEY\",",
            "  meshSecret: \"shared-secret\",",
            "  Activating token: \u{2581}our",
            "\"Create a WRITE token: https://example.org/settings/tokens7\",",
            "password: changeme1",
            "token: abc123 and more",
            "password:\nnext7",
        ] {
            assert_exact(input);
        }
    }

    /// A number under a password or a secret is a PIN; under a token or a key it is a count.
    #[test]
    fn digits_under_a_password_key_are_redacted() {
        let pin = ["1234", "5678"].concat();
        for (input, placeholder) in [
            (format!("password={pin}"), "<REDACTED:password>"),
            (format!("{{\"password\":\"{pin}\"}}"), "<REDACTED:password>"),
            (
                format!("      POSTGRES_PASSWORD: {pin}  # compose default"),
                "<REDACTED:password>",
            ),
            (format!("--db-password {pin}"), "<REDACTED:db_password>"),
            (format!("client_secret = {pin}"), "<REDACTED:client_secret>"),
        ] {
            assert_redacts(&input, &pin, placeholder);
        }
        for input in [
            "max_tokens=4096",
            "token=123456",
            "api_key=12345678",
            "password_length=1234",
            "secret_version: 2024",
            "password=123",
            "password=1234.5",
            "password=0x1234",
            "passwords=1234",
        ] {
            assert_exact(input);
        }
    }

    #[test]
    fn a_semicolon_inside_an_unquoted_value_is_covered_to_the_end_of_its_line() {
        let chosen = chosen();
        for (input, value) in [
            (
                format!("password={chosen};tail9"),
                format!("{chosen};tail9"),
            ),
            (
                format!("PASSWORD={chosen};a;b9\nNEXT=1"),
                format!("{chosen};a;b9"),
            ),
            (
                format!("password = {chosen};tail\u{e9}\r\n"),
                format!("{chosen};tail\u{e9}"),
            ),
            // `;` before a field, a statement, or the line's end closes the value.
            (
                format!("Server=db;Password={chosen};Database=app"),
                chosen.clone(),
            ),
            (
                format!("export PASSWORD={chosen}; echo done"),
                chosen.clone(),
            ),
            (format!("PASSWORD={chosen};echo done"), chosen.clone()),
            (format!("password={chosen};"), chosen.clone()),
        ] {
            assert_redacts(&input, &value, "<REDACTED:password>");
        }
    }

    /// The placeholder gate reads the text before the `;`, so the finding stands when a stand-in word follows it.
    #[test]
    fn the_placeholder_gate_reads_the_text_before_a_semicolon() {
        let chosen = chosen();
        let input = format!("password={chosen};placeholder");
        assert_redacts(
            &input,
            &format!("{chosen};placeholder"),
            "<REDACTED:password>",
        );
    }

    /// `KEY=` above another assignment, or before several words, is an empty assignment.
    #[test]
    fn an_empty_assignment_leaves_the_following_text_exact() {
        for input in [
            "api_token= next7 word",
            "api_token=\nnext7_line here",
            "API_TOKEN=\nDATABASE_URL=postgres://localhost/db7",
            "API_TOKEN=\r\nNEXT_URL=\r\n",
            "SERVICE_ROLE_KEY=        # set in the dashboard",
            "const maxTokens =\n\t\tbaseMaxTokens7 === undefined",
            "if (e.key === \"Enter\" || token == other7) {}",
            "assert cache.n_tokens == 4",
        ] {
            assert_exact(input);
        }
    }

    /// A value detached from its `=` by spaces or by a line break is the last word of its
    /// line: a wrapped assignment, or a key and its value on adjacent lines of output.
    #[test]
    fn a_value_detached_from_its_assignment_is_redacted_at_the_end_of_its_line() {
        let issued = issued();
        for input in [
            format!("api_token={issued}"),
            format!("api_token = {issued} trailing words"),
            format!("api_token\t=\t{issued}"),
            format!("api_token\n= {issued} trailing words"),
            format!("api_token= {issued}"),
            format!("api_token=\n{issued}\nnext7 line"),
            format!("api_token=\r\n    {issued}\r\n"),
            format!("api_token\n=\n{issued}\n"),
            format!("api_token= \"{issued}\""),
            format!("const api_token =\n    \"{issued}\";"),
        ] {
            assert_redacts(&input, &issued, "<REDACTED:api_token>");
        }
        let padded = format!("{issued}==");
        assert_redacts(
            &format!("password={padded}"),
            &padded,
            "<REDACTED:password>",
        );
    }

    #[test]
    fn a_name_that_ends_in_password_is_a_password_key() {
        let chosen = chosen();
        assert_redacts(
            &format!("PGPASSWORD={chosen} psql -h db app"),
            &chosen,
            "<REDACTED:password>",
        );
        assert_eq!(secret_key_label("PGPASSWORD").as_deref(), Some("password"));
        assert_eq!(
            secret_key_label("db_keystorepassword").as_deref(),
            Some("db_password")
        );
        assert!(secret_shaped_json_key("pgpassword"));
        assert_exact("passwordless=enabled7");
        assert!(!secret_shaped_json_key("passwordless"));
    }

    #[test]
    fn forms_the_redactor_already_covered_stay_covered() {
        let (issued, chosen) = (issued(), chosen());
        for (input, value, placeholder) in [
            (
                format!("gh auth login --token={issued}"),
                issued.clone(),
                "<REDACTED:token>",
            ),
            (
                format!("Server=db;Database=app;User Id=sa;Password={chosen};"),
                chosen.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("jdbc:mysql://db/app?user=sa&password={chosen}"),
                chosen.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("//registry.npmjs.org/:_authToken={issued}"),
                issued.clone(),
                "<REDACTED:auth_token>",
            ),
        ] {
            assert_redacts(&input, &value, placeholder);
        }
    }

    /// `head` and output limits cut a key block before its footer.
    #[test]
    fn private_key_body_lines_are_redacted_ahead_of_a_footer() {
        let (header, line) = (private_key_header(), private_key_body_line());
        let body = format!("{line}\n{line}\n{line}");
        assert_redacts(
            &format!("$ head -4 id_rsa\n{header}\n{body}\n$ "),
            &body,
            "<REDACTED:private_key>",
        );
        let escaped = format!("{line}\\n{line}");
        assert_redacts(
            &format!("printf '{header}\\n{escaped}\\n' > id_rsa"),
            &escaped,
            "<REDACTED:private_key>",
        );
        assert_exact(&format!("{header}\n...\n"));
        assert_exact(&format!("the file starts with {header} and a body"));
    }

    /// Filler lines of plain words, with `line` starting at `target_offset` exactly.
    fn text_with_line_at(target_offset: usize, line: &str, total: usize) -> (String, usize) {
        const FILLER: &str = "plain filler line with nothing to report 0123\n";
        let mut text = String::with_capacity(total + line.len() + 2 * FILLER.len());
        while text.len() + 2 * FILLER.len() <= target_offset {
            text.push_str(FILLER);
        }
        // One padded line brings the next line start to the target.
        text.push_str(&"x".repeat(target_offset - text.len() - 1));
        text.push('\n');
        let offset = text.len();
        assert_eq!(offset, target_offset);
        text.push_str(line);
        text.push('\n');
        while text.len() < total {
            text.push_str(FILLER);
        }
        (text, offset)
    }

    /// Each form is reported once, whole, wherever a window edge falls: cut by the first
    /// window's end, inside its edge margin, and around the second window's start.
    #[test]
    fn each_form_is_redacted_once_across_window_boundaries() {
        let (issued, chosen) = (issued(), chosen());
        let basic = basic_credentials();
        let key_body = format!("{0}\n{0}\n{0}", private_key_body_line());
        let forms = [
            (
                format!("deploy --password {issued} --verbose"),
                issued.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("  access_token: {issued}"),
                issued.clone(),
                "<REDACTED:access_token>",
            ),
            (
                format!(
                    "clone {}",
                    url("https", "alice", &issued, "example.org/r.git")
                ),
                issued.clone(),
                "<REDACTED:url_password>",
            ),
            (
                format!("Authorization: Basic {basic}"),
                basic.clone(),
                "<REDACTED:basic>",
            ),
            (
                format!("mysql -h db -uroot -p{issued} app"),
                issued.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("docker login -u bob -p {issued} registry.internal"),
                issued.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("DB_PASSWORD={chosen};{issued}"),
                format!("{chosen};{issued}"),
                "<REDACTED:db_password>",
            ),
            (
                format!("{}\n{key_body}", private_key_header()),
                key_body.clone(),
                "<REDACTED:private_key>",
            ),
            (
                format!("api_token=\n{issued}"),
                issued.clone(),
                "<REDACTED:api_token>",
            ),
        ];
        let window = MAX_REDACTABLE_BYTES;
        let second_start = window - WINDOW_OVERLAP_BYTES;
        let redactor = Redactor::new().unwrap();
        for (line, value, placeholder) in &forms {
            let value_at = line.find(value.as_str()).unwrap();
            let cut_by_the_first_window = window - value_at - value.len() / 2;
            for target in [
                cut_by_the_first_window,
                window - EDGE_MARGIN_BYTES / 2,
                second_start,
                second_start + EDGE_MARGIN_BYTES - 1,
                second_start + EDGE_MARGIN_BYTES + 1,
            ] {
                let (text, offset) = text_with_line_at(target, line, window + window / 2);
                let first_end = scan_windows(&text).next().unwrap().1;
                assert!(scan_windows(&text).count() > 1);
                if target == cut_by_the_first_window {
                    let value_start = offset + value_at;
                    assert!(
                        value_start < first_end && first_end < value_start + value.len(),
                        "{placeholder}: the first window ends inside the value"
                    );
                }
                let redaction = redactor.redact_windowed(&text, usize::MAX).unwrap();
                assert_eq!(
                    redaction.detections.len(),
                    1,
                    "{placeholder} at {target}: {:?}",
                    redaction.detections
                );
                assert_eq!(redaction.detections[0].offset, offset + value_at);
                assert_eq!(redaction.detections[0].length, value.len());
                assert_eq!(
                    redaction.text,
                    text.replacen(value.as_str(), placeholder, 1),
                    "{placeholder} at {target}"
                );
                assert_eq!(redactor.detect_windowed(&text), Ok(true));
            }
        }
    }

    /// The first window ends inside the value of a field line, where the line gate reads the
    /// cut as the end of the input. The cut prefix is left to the second window, which reads
    /// the line whole.
    #[test]
    fn a_window_end_inside_a_field_value_reports_the_whole_value() {
        let issued = issued();
        let value = format!("{issued}{}", "x7".repeat(64));
        let line = format!("password: {value}");
        let window = MAX_REDACTABLE_BYTES;
        let (text, offset) = text_with_line_at(window - line.len() / 2, &line, window + window / 2);
        assert!(offset + "password: ".len() < window && offset + line.len() > window);
        let redaction = Redactor::new()
            .unwrap()
            .redact_windowed(&text, usize::MAX)
            .unwrap();
        assert_eq!(redaction.detections.len(), 1);
        assert_eq!(redaction.detections[0].length, value.len());
        assert_eq!(
            redaction.text,
            text.replacen(value.as_str(), "<REDACTED:password>", 1)
        );
    }

    /// Repeats `unit` up to `total` bytes.
    fn filled(unit: &str, total: usize) -> String {
        unit.repeat(total / unit.len())
    }

    /// Inputs built to make each form's pattern do the most work per byte: a key missing its
    /// value, a program name missing its option, an authority missing its `@`, a value that
    /// runs past its line's end, a key behind a long dotted prefix, and a `;` chain that ends
    /// in `=`.
    fn adversarial_inputs(total: usize) -> Vec<(&'static str, String)> {
        let body_line = format!("{}\n", private_key_body_line());
        vec![
            ("option without a value", filled("--password \t", total)),
            ("option after option", filled("--token --token a ", total)),
            ("program without an option", filled("mysql ", total)),
            (
                "login without an option",
                filled("docker x login y ", total),
            ),
            ("authority without userinfo", filled("x://u:p", total)),
            (
                "authority of colons",
                format!("x://u:{}", filled("p:", total - 8)),
            ),
            (
                "field value that never ends its line",
                format!("token: {}\"", filled("a1", total - 16)),
            ),
            ("fields on one line", filled("token: a1 ", total)),
            (
                "dotted key prefix",
                format!("{}token: x1 y", filled("a.", total - 16)),
            ),
            (
                "private key body",
                format!(
                    "{}\n{}",
                    private_key_header(),
                    filled(&body_line, total - 64)
                ),
            ),
            (
                "private key headers",
                filled(&format!("{}\n", private_key_header()), total),
            ),
            (
                "semicolon chain that ends in a pair",
                format!("password=a{}=", filled(";b", total - 16)),
            ),
            (
                "semicolon chains",
                filled("k_token=a;b;c;d;e;f;g;h ", total),
            ),
            (
                "scheme without credentials",
                filled("Authorization: Basic ", total),
            ),
            (
                "spaced assignment without a value",
                filled("token \t = \t", total),
            ),
        ]
    }

    /// The work the scanner charges to its budget doubles when an adversarial input doubles,
    /// within an eighth for the context windows at the edges, and the largest single scan
    /// stays inside the default budget: it either completes or fails closed on the
    /// match-length bound. The charge covers each rule's pass over the input and each
    /// candidate's context window; the patterns themselves run on a linear-time automaton.
    #[test]
    fn adversarial_input_costs_linear_work() {
        let redactor = Redactor::new().unwrap();
        let full = adversarial_inputs(MAX_REDACTABLE_BYTES);
        let half = adversarial_inputs(MAX_REDACTABLE_BYTES / 2);
        for ((name, full), (_, half)) in full.iter().zip(&half) {
            let full_report = redactor.scanner.scan(full).unwrap();
            let half_report = redactor.scanner.scan(half).unwrap();
            assert!(
                matches!(full_report.limits_hit, None | Some(LimitExhausted::Match)),
                "{name}: {:?}",
                full_report.limits_hit
            );
            assert!(
                full_report.work_bytes <= 2 * half_report.work_bytes + half_report.work_bytes / 8,
                "{name}: {} bytes of work for {} input bytes, {} for half",
                full_report.work_bytes,
                full.len(),
                half_report.work_bytes
            );
            match redactor.redact(full) {
                Ok(redaction) => assert!(redaction.text.len() <= 20 * full.len(), "{name}"),
                Err(error) => assert_eq!(error.kind(), RedactionErrorKind::MatchLimit, "{name}"),
            }
        }
    }

    /// The windowed walk scans a multi-megabyte adversarial text window by window under the
    /// same budget.
    #[test]
    fn windowed_redaction_completes_on_a_long_adversarial_text() {
        let redactor = Redactor::new().unwrap();
        let mut text = String::new();
        // Each piece is shorter than the match-length bound, so every match stays inside it.
        // The corpus rule reads from a private-key header to the next footer, so the key pieces
        // come last and a footer closes them.
        // The `;` chain that stays open past the continuation bound fails closed and is left out.
        let (keys, others): (Vec<_>, Vec<_>) =
            adversarial_inputs(secret_scanner::MAX_MATCH_BYTES / 2)
                .into_iter()
                .filter(|(name, _)| !name.starts_with("semicolon chain that ends"))
                .partition(|(name, _)| name.starts_with("private key"));
        for (_, input) in others.iter().chain(&keys) {
            text.push_str(input);
            text.push('\n');
        }
        text.push_str(&private_key_header().replace("BEGIN", "END"));
        text.push('\n');
        let text = text.repeat(5);
        assert!(text.len() > 2 * MAX_REDACTABLE_BYTES);
        let detections = redactor
            .redact_windowed(&text, usize::MAX)
            .unwrap()
            .detections;
        assert!(!detections.is_empty());
        assert_eq!(redactor.detect_windowed(&text), Ok(true));
    }

    /// A key's own text can be part of another finding, so a label holds vocabulary words only.
    #[test]
    fn a_label_holds_no_text_of_a_password_suffix_key() {
        let input = url("https", "alice", "r9q8password=Z7v", "host/");
        let redaction = redact_durable_text(&input);
        assert!(!redaction.text.contains("r9q8"), "{}", redaction.text);
        assert!(!redaction.text.contains("Z7v"), "{}", redaction.text);
        assert!(!redaction.detections.is_empty());
        for detection in &redaction.detections {
            assert!(!detection.secret_type.contains("r9q8"), "{detection:?}");
        }
    }

    #[test]
    fn a_detached_value_keeps_its_padding_its_semicolon_and_its_comment_outside() {
        let chosen = chosen();
        let padded = ["QUJDRDEyMzQ1", "Ng=="].concat();
        for (input, value) in [
            (format!("API_TOKEN=\n{padded}\nnext line"), padded.clone()),
            (format!("API_TOKEN= {padded}"), padded.clone()),
            (format!("API_TOKEN= {chosen}  # rotate me"), chosen.clone()),
            (
                format!("API_TOKEN=\n{chosen};tail9\n"),
                format!("{chosen};tail9"),
            ),
        ] {
            assert_redacts(&input, &value, "<REDACTED:api_token>");
        }
        for input in [
            "API_TOKEN=\n}",
            "API_TOKEN=\n[section]",
            "API_TOKEN=\n---",
            "API_TOKEN=\n#comment7",
            "api_token=\ncfg.auth_token",
        ] {
            assert_exact(input);
        }
    }

    /// Serialized text writes a line break as the two characters `\n`.
    #[test]
    fn line_based_forms_read_an_escaped_line_break_as_a_line_end() {
        let (issued, chosen) = (issued(), chosen());
        for (input, value, placeholder) in [
            (
                format!("{{\"text\":\"user: bob\\npassword: {chosen}\\nnext: line\"}}"),
                chosen.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("first line\\ntoken: {issued}\\r\\nrest"),
                issued.clone(),
                "<REDACTED:token>",
            ),
            (
                format!("out\\napi_token= {issued}\\nnext line"),
                issued.clone(),
                "<REDACTED:api_token>",
            ),
            (
                format!("out\\nPASSWORD={chosen};tail9\\nnext line"),
                format!("{chosen};tail9"),
                "<REDACTED:password>",
            ),
        ] {
            assert_redacts(&input, &value, placeholder);
        }
    }

    #[test]
    fn a_private_key_body_is_read_under_armor_headers_and_indentation() {
        let line = private_key_body_line();
        let body = format!("{line}\n{line}");
        let pgp = format!("-----BEGIN PGP {} KEY BLOCK-----", "PRIVATE");
        for input in [
            format!("{pgp}\nVersion: GnuPG v2\n\n{body}\n"),
            format!("{pgp}\n\n{body}\n"),
            format!(
                "{}\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,0A1B2C3D\n\n{body}\n",
                private_key_header()
            ),
        ] {
            assert_redacts(&input, &body, "<REDACTED:private_key>");
        }
        let indented = format!("{line}\n    {line}");
        assert_redacts(
            &format!(
                "  private_key: |\n    {}\n    {indented}\n",
                private_key_header()
            ),
            &indented,
            "<REDACTED:private_key>",
        );
        assert_exact(&format!("{pgp}\nVersion: GnuPG v2\n\n...\n"));
    }

    /// The `;` walk reads a fixed span past the value, so a window end beyond that span changes
    /// nothing: a tail a following word disproves gives the same head-only span in the whole
    /// line and across windows, and a tail still open at the bound fails both scans closed.
    #[test]
    fn a_semicolon_tail_cut_by_a_window_end_gets_the_verdict_of_the_whole_line() {
        let chosen = chosen();
        let redactor = Redactor::new().unwrap();
        let window = MAX_REDACTABLE_BYTES;
        let rest = "x7 ".repeat(EDGE_MARGIN_BYTES);
        let line = format!("password={chosen};tail9 and {rest}");
        assert_eq!(
            redactor.redact(&line).unwrap().text,
            format!("password=<REDACTED:password>;tail9 and {rest}")
        );
        let target = window - "password=".len() - chosen.len() - EDGE_MARGIN_BYTES - 64;
        let (text, offset) = text_with_line_at(target, &line, window + window / 2);
        let first_end = scan_windows(&text).next().unwrap().1;
        assert!(offset + line.len() > first_end);
        let redaction = redactor.redact_windowed(&text, usize::MAX).unwrap();
        assert_eq!(redaction.detections.len(), 1);
        assert_eq!(redaction.detections[0].offset, offset + "password=".len());
        assert_eq!(redaction.detections[0].length, chosen.len());
        assert_eq!(redactor.detect_windowed(&text), Ok(true));

        let open = format!("password={chosen};{}", "x7".repeat(EDGE_MARGIN_BYTES));
        assert_eq!(
            redactor.redact(&open).unwrap_err().kind(),
            RedactionErrorKind::MatchLimit
        );
        let (text, _) = text_with_line_at(target, &open, window + window / 2);
        assert_eq!(
            redactor
                .redact_windowed(&text, usize::MAX)
                .unwrap_err()
                .kind(),
            RedactionErrorKind::MatchLimit
        );
    }

    #[test]
    fn a_continued_value_is_judged_whole_and_ends_before_trailing_blanks() {
        // A member-path head, joined at run time as the other credential shapes are.
        let dotted = ["my.", "pass;w0rd"].concat();
        for (input, value) in [
            ("PASSWORD=12;a7B".to_owned(), "12;a7B"),
            (format!("password={dotted}"), dotted.as_str()),
            ("password=ab7;cd9 \t\nnext".to_owned(), "ab7;cd9"),
        ] {
            assert_redacts(&input, value, "<REDACTED:password>");
        }
        assert_exact("PASSWORD=12; echo done");
        assert_exact(&format!("monkey=ab7;{}", "x7".repeat(40_000)));
    }

    #[test]
    fn basic_credentials_in_another_charset_are_redacted() {
        // The Base64 of `u:caf` and the Latin-1 byte 0xE9.
        let latin = ["dTpj", "YWbp"].concat();
        assert_redacts(
            &format!("Authorization: Basic {latin}"),
            &latin,
            "<REDACTED:basic>",
        );
    }

    #[test]
    fn a_negated_switch_takes_no_value() {
        assert_exact("pg_dump -U app --no-password app_db_2");
        assert_redacts(
            "pg_dump -U app --password app_db_2",
            "app_db_2",
            "<REDACTED:password>",
        );
    }

    #[test]
    fn a_field_value_keeps_its_symbols_and_leaves_its_comment_to_the_scan() {
        let chosen = chosen();
        let symbols = ["Pa$$w", "0rd9"].concat();
        assert_redacts(
            &format!("password: {symbols}"),
            &symbols,
            "<REDACTED:password>",
        );
        assert_redacts(
            &format!("password: changeme # prod password: {chosen}"),
            &chosen,
            "<REDACTED:password>",
        );
        assert_redacts("password: Welcome7!", "Welcome7!", "<REDACTED:password>");
        // A comment longer than the match bound stays outside the match.
        assert_exact(&format!("password: string # {}", "note ".repeat(8_000)));
        for input in [
            "  password: String!",
            "  password: $DB_PASSWORD",
            "token: cfg.auth_token",
            "tool --token cfg.auth_token",
        ] {
            assert_exact(input);
        }
    }

    #[test]
    fn url_userinfo_reads_an_encoded_first_octet_and_a_user_name_with_an_at_sign() {
        let issued = issued();
        let encoded = format!("%40{issued}");
        assert_redacts(
            &url("postgres", "app", &encoded, "db.internal/app"),
            &encoded,
            "<REDACTED:url_password>",
        );
        assert_redacts(
            &url("smtp", "user@example.com", &issued, "mail.internal:587"),
            &issued,
            "<REDACTED:url_password>",
        );
        assert_exact(&url("https", "u", "%DB_PASSWORD%", "host/"));
    }

    /// The program is the command, `login` is its subcommand, and `sshpass` takes `-p` first.
    #[test]
    fn a_client_password_option_belongs_to_its_own_command() {
        let (issued, chosen) = (issued(), chosen());
        let pin = ["1234", "5678"].concat();
        for (input, value) in [
            (
                format!("sshpass -p{chosen} ssh deploy@host"),
                chosen.clone(),
            ),
            (
                format!("docker login -u bob -p{issued} registry.internal"),
                issued.clone(),
            ),
            (
                format!("crane auth login registry.internal -u bob -p {issued}"),
                issued.clone(),
            ),
            (format!("sudo mysql -uroot -p{pin} app"), pin.clone()),
            (format!("sshpass -p {pin} ssh deploy@host"), pin.clone()),
        ] {
            assert_redacts(&input, &value, "<REDACTED:password>");
        }
        for input in [
            "docker run --name login -p 8080:80 image7",
            "docker run -d --name login-service -p 8443:443 image7",
            "cf push login -p target/app-1.0.jar",
            "find /var/lib/mysql -print0",
            "docker run --name mysql -p3306:3306 image7",
            "ssh mysql-prod -p2222",
            "sshpass -e ssh -p 2222 host",
            "mysql -uroot -p123 app",
        ] {
            assert_exact(input);
        }
    }

    /// Source code wraps an assignment after `=` and ends the statement with `;`.
    #[test]
    fn a_wrapped_statement_and_a_closing_bracket_stay_exact() {
        for input in [
            "const accessToken =\n  response.data.access_token;",
            "this.apiKey =\n      options.apiKey;",
            "const MAX_TOKENS =\n  4096;",
            "{useAuth=false;}",
            "{this.apiKey=opts.apiKey;}",
            // The corpus rule's own gate still reads a quoted word of letters as a name.
            "{ provider: \"anthropic\", api: \"anthropic-messages\" }",
            "API_TOKEN=\nURL=\n",
            "token=\nurl=\n",
            "API_TOKEN=\r\nAPI_KEY=\r\n",
            "jdbc:sqlserver://contoso.database.windows.net:1433;database=sales;user=appuser@contoso;password=${DB_PASSWORD};encrypt=true;",
        ] {
            assert_exact(input);
        }
        let chosen = chosen();
        assert_redacts(
            &format!("API_TOKEN=\n{chosen};\n"),
            &chosen,
            "<REDACTED:api_token>",
        );
    }

    #[test]
    fn a_value_of_digits_dots_stars_or_percent_signs_is_a_password_where_one_is_named() {
        // Each row is the text before the value, the value, and the text after it, joined at
        // run time so each line of this file stays clear of a credential shape.
        let cases = [
            (
                "sshpass -p ",
                "19.04.1987",
                " ssh deploy@host",
                "<REDACTED:password>",
            ),
            (
                "tool --password '",
                "%7qW9%",
                "' run",
                "<REDACTED:password>",
            ),
            ("PASSWORD=\n", "Q7***r9", "\n", "<REDACTED:password>"),
            (
                "deploy --password ",
                "String!",
                " now",
                "<REDACTED:password>",
            ),
            (
                "deploy --password '",
                "river.blue7",
                "'",
                "<REDACTED:password>",
            ),
            ("password: \"", "river.blue7", "\"", "<REDACTED:password>"),
            ("password: ", "-Q7r9x", "", "<REDACTED:password>"),
            (
                "sshpass -p",
                "-a9b7",
                " ssh deploy@host",
                "<REDACTED:password>",
            ),
            ("password=", "ab7;cd9", " # note", "<REDACTED:password>"),
            (
                "API_TOKEN= ",
                "hunter2x;placeholder",
                "",
                "<REDACTED:api_token>",
            ),
            ("password=\n", "Tr0ub4dor", "&3\n", "<REDACTED:password>"),
            ("FERNET_KEY=\n", "q7Lm-2Xv_9Rt4Kp=", "\n", "<REDACTED:key>"),
        ];
        for (before, value, after, placeholder) in cases {
            assert_redacts(&format!("{before}{value}{after}"), value, placeholder);
        }
        assert_redacts(
            &url("https", "alice", "*Q7r9", "db.internal/"),
            "*Q7r9",
            "<REDACTED:url_password>",
        );
        assert_redacts(
            &url("https", "alice", ":Q7r9", "db.internal/"),
            ":Q7r9",
            "<REDACTED:url_password>",
        );
    }

    #[test]
    fn serialized_text_keeps_adjacent_fields_commands_and_key_bodies() {
        let (issued, chosen) = (issued(), chosen());
        let two_fields = format!("password: {chosen}\\ntoken: {issued}\\nrest");
        let redaction = redact_durable_text(&two_fields);
        assert_eq!(
            redaction.text,
            "password: <REDACTED:password>\\ntoken: <REDACTED:token>\\nrest"
        );
        let basic = basic_credentials();
        let line = private_key_body_line();
        let escaped_body = format!("{line}\\r\\n{line}");
        for (input, value, placeholder) in [
            (
                format!("x\\u000apassword: {chosen}\\u000anext"),
                chosen.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("out\\nmysql -uroot -p{chosen} app"),
                chosen.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("HTTP/1.1\\r\\nAuthorization: Basic {basic}\\r\\n"),
                basic.clone(),
                "<REDACTED:basic>",
            ),
            (
                format!("/usr/bin/mysqldump -uroot -p{chosen} app"),
                chosen.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("mysql.exe -uroot -p{chosen}"),
                chosen.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("{}\\r\\n{escaped_body}\\r\\n", private_key_header()),
                escaped_body.clone(),
                "<REDACTED:private_key>",
            ),
        ] {
            assert_redacts(&input, &value, placeholder);
        }
        // `echo user:password | base64` encodes the line break too.
        let with_line_break = ["YWxpY2U6", "aHVudGVyMgo="].concat();
        assert_redacts(
            &format!("Authorization: Basic {with_line_break}"),
            &with_line_break,
            "<REDACTED:basic>",
        );
    }

    /// A value detached from its `=` is reported where the delimiter that ends it stands
    /// directly after it: the quote that closes the string around it, a bracket, `;`, `|`.
    #[test]
    fn a_detached_value_is_reported_before_the_delimiter_that_ends_it() {
        let chosen = chosen();
        for (before, after) in [
            ("msg=\"login password= ", "\" user=bob"),
            ("(password= ", ")"),
            ("password= ", "; echo done"),
            ("password=\n", "|cat"),
            ("API_TOKEN=\n", ">"),
            ("token= ", " \\\n  --flag"),
            ("password=\t", "\t\n"),
        ] {
            let label = if before.contains("TOKEN") || before.contains("token") {
                if before.contains("API") {
                    "<REDACTED:api_token>"
                } else {
                    "<REDACTED:token>"
                }
            } else {
                "<REDACTED:password>"
            };
            assert_redacts(&format!("{before}{chosen}{after}"), &chosen, label);
        }
    }

    /// Text after an empty assignment stays as written in two shapes: a word that another
    /// word follows on its line, and a word that is an assignment of its own.
    #[test]
    fn a_word_after_an_empty_assignment_that_starts_other_text_stays_exact() {
        let chosen = chosen();
        for input in [
            format!("api_token= {chosen} --verbose"),
            format!("api_token=\n{chosen} and more words"),
            "API_KEY= run command".to_owned(),
            "API_TOKEN=\nDATABASE_URL=postgres://localhost/db".to_owned(),
            "API_TOKEN=\nURL=".to_owned(),
        ] {
            assert_exact(&input);
        }
    }

    /// The assignment below an empty one is read with its own key, whichever way its value
    /// is attached.
    #[test]
    fn an_assignment_below_an_empty_one_is_read_with_its_own_key() {
        let chosen = chosen();
        for line in [
            format!("DB_PASSWORD= {chosen}"),
            format!("DB_PASSWORD={chosen}"),
            format!("DB_PASSWORD = {chosen}"),
        ] {
            assert_redacts(
                &format!("API_TOKEN=\n{line}\n"),
                &chosen,
                "<REDACTED:db_password>",
            );
        }
        assert_redacts(
            &format!("secret: password: {chosen}"),
            &chosen,
            "<REDACTED:password>",
        );
    }

    /// Serialized text writes a line break as `\n`, which ends the shell word an option or a
    /// client's `-p` takes.
    #[test]
    fn an_option_value_ends_at_an_escaped_line_break() {
        let (issued, chosen) = (issued(), chosen());
        for (input, value, placeholder) in [
            (
                format!("deploy --password {issued}\\nnext line"),
                issued.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("tool -token {issued}\\r\\nrest"),
                issued.clone(),
                "<REDACTED:token>",
            ),
            (
                format!("mysql -uroot -p{chosen}\\nnext line"),
                chosen.clone(),
                "<REDACTED:password>",
            ),
            (
                format!("sshpass -p {chosen}\\nssh deploy@host"),
                chosen.clone(),
                "<REDACTED:password>",
            ),
        ] {
            assert_redacts(&input, &value, placeholder);
        }
    }

    /// Blanks after a line-final value stay outside the match, so a run of them longer than
    /// the match bound leaves the verdict as it is, whole or in windows.
    #[test]
    fn blank_padding_after_a_line_final_value_stays_outside_the_match() {
        let chosen = chosen();
        let padding = " ".repeat(secret_scanner::MAX_MATCH_BYTES + 1024);
        for (line, placeholder) in [
            (
                format!("password: {chosen}{padding}"),
                "<REDACTED:password>",
            ),
            (
                format!("DB_PASSWORD= {chosen}{padding}"),
                "<REDACTED:db_password>",
            ),
        ] {
            assert_redacts(&format!("{line}\nnext line\n"), &chosen, placeholder);
            // The first window ends inside the padding.
            let window = MAX_REDACTABLE_BYTES;
            let (text, offset) =
                text_with_line_at(window - padding.len() / 2, &line, window + window / 2);
            assert!(offset + line.len() - padding.len() < window && offset + line.len() > window);
            let redaction = redact_windowed_durable_text(&text, usize::MAX).unwrap();
            assert_eq!(
                redaction.text,
                text.replacen(chosen.as_str(), placeholder, 1)
            );
        }
        // A clean field line with the same padding carries no finding in either reading.
        let line = format!("password: string{padding}");
        assert_exact(&format!("{line}\nnext line\n"));
        let window = MAX_REDACTABLE_BYTES;
        let (text, _) = text_with_line_at(window - padding.len() / 2, &line, window + window / 2);
        let redaction = redact_windowed_durable_text(&text, usize::MAX).unwrap();
        assert_eq!(redaction.text, text);
    }

    /// A line gets the redaction it gets alone wherever the edges of the scan windows fall
    /// around it: at the first window's end, at that window's edge margin, at the second
    /// window's start, and at the second window's edge margin.
    #[test]
    fn windowed_redaction_agrees_with_the_redaction_of_a_line_alone_at_every_window_edge() {
        const FILLER: &str = "plain filler line with nothing to report 0123\n";
        let (issued, chosen) = (issued(), chosen());
        let lines = [
            format!("password={chosen}"),
            format!("api_key = {issued};tail9"),
            format!("DB_PASSWORD= {chosen}"),
            format!("DB_PASSWORD=\n{chosen}"),
            format!("msg=\"login password= {chosen}\" user=bob"),
            format!("api_token= {chosen} --verbose"),
            "API_TOKEN=\nDATABASE_URL=postgres://localhost/db".to_owned(),
            format!("API_TOKEN=\nDB_PASSWORD= {chosen}"),
            format!("deploy --password {chosen} --verbose"),
            format!("  password: {chosen}  # note"),
            format!("password: {chosen}{}", " ".repeat(3000)),
            format!("X-Api-Key: {issued}"),
            url("https", "alice", &chosen, "example.org/r.git"),
            format!("mysql -uroot -p{chosen} app"),
            format!("sshpass -p {chosen} ssh deploy@host"),
            format!("Authorization: Basic {}", basic_credentials()),
            format!("out\\napi_token= {issued}\\nnext line"),
            format!(
                "{}\n{}",
                private_key_header(),
                [private_key_body_line(), private_key_body_line()].join("\n")
            ),
        ];
        let window = MAX_REDACTABLE_BYTES;
        let (mut left_to_second, mut kept_by_first) = (0, 0);
        for line in &lines {
            let alone = redact_durable_text(&format!("{FILLER}{line}\n{FILLER}")).text;
            let redacted = &alone[FILLER.len()..alone.len() - FILLER.len() - 1];
            let size = line.len();
            for edge in [
                window - EDGE_MARGIN_BYTES,
                window,
                window - WINDOW_OVERLAP_BYTES,
                window - WINDOW_OVERLAP_BYTES + EDGE_MARGIN_BYTES,
            ] {
                for offset in [
                    edge - size - FILLER.len(),
                    edge - size - 2,
                    edge - size,
                    edge - size / 2,
                    edge - 1,
                    edge,
                    edge + 1,
                    edge + FILLER.len(),
                ] {
                    let (text, start) = text_with_line_at(offset, line, window + window / 2);
                    let (second_start, _) = scan_windows(&text).nth(1).unwrap();
                    if start + size > window - EDGE_MARGIN_BYTES {
                        left_to_second += 1;
                    } else if start < second_start + EDGE_MARGIN_BYTES {
                        kept_by_first += 1;
                    }
                    let windowed = redact_windowed_durable_text(&text, usize::MAX).unwrap();
                    assert_eq!(
                        windowed.text,
                        text.replacen(line.as_str(), redacted, 1),
                        "{line:?} at {offset}"
                    );
                }
            }
        }
        // The sweep puts lines on both sides of each window's keep boundary.
        assert!(left_to_second >= lines.len() && kept_by_first >= lines.len());
    }

    /// The `-p` of a `login` client follows the subcommand within 256 bytes of one command.
    #[test]
    fn a_client_option_is_read_within_the_gap_its_rule_bounds() {
        let chosen = chosen();
        let command = |gap: usize| {
            format!(
                "docker login {} -p {chosen} registry.io",
                "x".repeat(gap - 1)
            )
        };
        assert_redacts(&command(256), &chosen, "<REDACTED:password>");
        assert_exact(&command(257));
    }
}

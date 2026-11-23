use std::sync::LazyLock;

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, AhoCorasickKind, MatchKind};
use regex::bytes::Captures;

use crate::api::REVISION;
use crate::rules::{
    CodeReferenceGate, EntropySpec, LineTail, LocalContextSpec, OfflineValidationKind,
    OfflineValidationSpec, Rule, RuleSet,
};
use crate::{
    Finding, LimitExhausted, MAX_LOCAL_CONTEXT_BYTES, MAX_MATCH_BYTES, RuleSource, ScanError,
    ScanLimits, ScanProfile, ScanReport, TextSpan,
};

#[derive(Debug)]
enum Abort {
    Work,
    Match,
    Invalid(ScanError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CandidateSpans {
    full: TextSpan,
    value: TextSpan,
    reported: TextSpan,
    key: Option<TextSpan>,
}

impl From<ScanError> for Abort {
    fn from(error: ScanError) -> Self {
        Self::Invalid(error)
    }
}

pub(crate) fn evaluate(
    rules: &RuleSet,
    profile: ScanProfile,
    limits: ScanLimits,
    semantic_digest: [u8; 32],
    input: &str,
) -> Result<ScanReport, ScanError> {
    if input.len() > limits.max_input_bytes {
        return Err(ScanError::InputLimitExceeded);
    }
    let bytes = input.as_bytes();
    let mut findings = Vec::new();
    let mut candidates = 0usize;
    let mut work = 0usize;
    let mut limits_hit = None;

    // Charge input bytes before preselection so `max_work_bytes` bounds preselection.
    if add_work(&mut work, bytes.len(), limits.max_work_bytes).is_err() {
        return Ok(ScanReport {
            findings,
            revision: REVISION,
            semantic_digest,
            candidates_evaluated: candidates,
            work_bytes: work,
            limits_hit: Some(LimitExhausted::Work),
        });
    }

    let input_is_ascii = input.is_ascii();

    'rules: for rule in rules.preselect(profile, bytes) {
        if add_work(&mut work, bytes.len(), limits.max_work_bytes).is_err() {
            limits_hit = Some(LimitExhausted::Work);
            break 'rules;
        }
        if let Some(byte) = rule.required_byte
            && memchr::memchr(byte, bytes).is_none()
        {
            continue;
        }
        if input_is_ascii && let Some(form) = KeyedForm::for_rule(&rule.declaration.name) {
            let mut cursor = 0;
            while let Some(at) =
                memchr::memchr(form.anchor(), &bytes[cursor..]).map(|offset| cursor + offset)
            {
                match form.probe(input, at, cursor)? {
                    Probe::Candidate(spans) => {
                        cursor = spans.full.end();
                        if candidates >= limits.max_candidates {
                            limits_hit = Some(LimitExhausted::Candidates);
                            break 'rules;
                        }
                        candidates += 1;
                        // `candidate_spans` counts an empty value as a candidate and skips it; the fast path must charge the same candidate budget.
                        if spans.value.is_empty() {
                            continue;
                        }
                        match evaluate_candidate_spans(rules, rule, spans, input, &mut work, limits)
                        {
                            Ok(finding) => {
                                if rule.resumes_at_value {
                                    cursor =
                                        resume_after(rule, input, spans, finding.as_ref(), cursor);
                                }
                                findings.extend(finding);
                            }
                            Err(Abort::Work) => {
                                limits_hit = Some(LimitExhausted::Work);
                                break 'rules;
                            }
                            Err(Abort::Match) => record_skipped_match(&mut limits_hit),
                            Err(Abort::Invalid(error)) => return Err(error),
                        }
                    }
                    Probe::Skip(next) => {
                        debug_assert!(next > at, "a probe must advance past its anchor");
                        cursor = next;
                    }
                }
            }
            continue;
        }
        if rule.resumes_at_value {
            // `API_TOKEN=` above `DB_PASSWORD= value` holds an assignment that is read with its
            // own key, so the search resumes inside a candidate where `resume_after` says.
            // Each resumed search starts past the previous one, so the walk ends, and the
            // bytes a resumed search reads again are charged.
            let mut at = 0;
            while let Some(captures) = rule.regex.captures_at(bytes, at) {
                if candidates >= limits.max_candidates {
                    limits_hit = Some(LimitExhausted::Candidates);
                    break 'rules;
                }
                candidates += 1;
                let match_end = captures.get(0).map_or(bytes.len(), |full| full.end());
                let mut resume = match_end;
                let outcome = candidate_spans(rule, &captures, input).and_then(|spans| {
                    let Some(spans) = spans else { return Ok(None) };
                    let finding =
                        evaluate_candidate_spans(rules, rule, spans, input, &mut work, limits)?;
                    resume = resume_after(rule, input, spans, finding.as_ref(), match_end);
                    Ok(finding)
                });
                let charged = add_work(
                    &mut work,
                    match_end.saturating_sub(resume),
                    limits.max_work_bytes,
                );
                match outcome.and_then(|finding| charged.map(|()| finding)) {
                    Ok(finding) => findings.extend(finding),
                    Err(Abort::Work) => {
                        limits_hit = Some(LimitExhausted::Work);
                        break 'rules;
                    }
                    Err(Abort::Match) => record_skipped_match(&mut limits_hit),
                    Err(Abort::Invalid(error)) => return Err(error),
                }
                at = resume.max(at + 1);
                if at > bytes.len() {
                    break;
                }
            }
            continue;
        }
        for captures in rule.regex.captures_iter(bytes) {
            if candidates >= limits.max_candidates {
                limits_hit = Some(LimitExhausted::Candidates);
                break 'rules;
            }
            candidates += 1;
            match evaluate_candidate(rules, rule, &captures, input, &mut work, limits) {
                Ok(Some(finding)) => findings.push(finding),
                Ok(None) => {}
                Err(Abort::Work) => {
                    limits_hit = Some(LimitExhausted::Work);
                    break 'rules;
                }
                Err(Abort::Match) => record_skipped_match(&mut limits_hit),
                Err(Abort::Invalid(error)) => return Err(error),
            }
        }
    }
    findings.sort_by(|left, right| {
        (
            left.full_span.start(),
            left.value_span.start(),
            left.rule_source,
            left.rule_id.as_str(),
            left.full_span.end(),
            left.value_span.end(),
        )
            .cmp(&(
                right.full_span.start(),
                right.value_span.start(),
                right.rule_source,
                right.rule_id.as_str(),
                right.full_span.end(),
                right.value_span.end(),
            ))
    });

    Ok(ScanReport {
        findings,
        revision: REVISION,
        semantic_digest,
        candidates_evaluated: candidates,
        work_bytes: work,
        limits_hit,
    })
}

/// Where a rule that resumes its search continues after a candidate whose match ends at
/// `match_end`.
///
/// A line-shaped rule (the detached assignment and the field):
/// - a rejected candidate resumes at its value, which may be an assignment of its own
///   (`DB_PASSWORD= value` below an empty `API_TOKEN=`);
/// - a finding cut short of the match, at an escaped line break, resumes at the cut, where
///   the next serialized line starts;
/// - a finding whose value ends in one `=` (`clientSecret=`) resumes at that value, so the
///   word is also read as a key.
///
/// The assignment rule resumes at the value of a candidate whose key stands after an empty
/// assignment (`db.password =` above `keycloak.url = https://sso/auth?client_secret=VALUE`):
/// the value may hold the assignment that carries the credential.
fn resume_after(
    rule: &Rule,
    input: &str,
    spans: CandidateSpans,
    finding: Option<&Finding>,
    match_end: usize,
) -> usize {
    let bytes = input.as_bytes();
    if rule.line_tail.is_none() {
        let key_start = spans.key.map_or(spans.full.start(), |key| key.start());
        return if follows_an_empty_assignment(input, key_start) {
            spans.value.start()
        } else {
            match_end
        };
    }
    let Some(finding) = finding else {
        return spans.value.start();
    };
    if finding.full_span.end() < match_end {
        return finding.full_span.end();
    }
    let run = &bytes[spans.value.start()..spans.value.end()];
    if run.len() > 1 && run.ends_with(b"=") && !run.ends_with(b"==") {
        spans.value.start()
    } else {
        match_end
    }
}

/// Whether the key at `key_start` lies in the run that stands where the value of an
/// assignment would: after a secret-shaped key, one `=`, and a gap that detaches a value
/// (blanks after an `=` that follows its key directly, or a line break). The run may open
/// with a few bytes before the key (`#key`, `+key`, `13:key`).
fn follows_an_empty_assignment(input: &str, key_start: usize) -> bool {
    let bytes = input.as_bytes();
    let floor = key_start.saturating_sub(MAX_ESCAPE_RUN_BYTES);
    let mut run_start = key_start;
    while run_start > floor
        && bytes[run_start - 1].is_ascii()
        && !is_rule_space_byte(bytes[run_start - 1])
        && !is_value_terminator(bytes[run_start - 1])
    {
        run_start -= 1;
    }
    let Some(before) = input.get(..run_start) else {
        return false;
    };
    let assignment = before.trim_end_matches(is_rule_space);
    let gap = &before[assignment.len()..];
    let Some(equals) = assignment
        .len()
        .checked_sub(1)
        .filter(|at| bytes[*at] == b'=')
    else {
        return false;
    };
    let Some(&key_end) = equals.checked_sub(1).and_then(|at| bytes.get(at)) else {
        return false;
    };
    let detached = gap.contains(['\n', '\r']) || (!gap.is_empty() && is_key_byte(key_end));
    key_end != b'=' && detached && assignment_key(bytes, equals, 0).is_some()
}

/// The bytes that end an unquoted value run.
fn is_value_terminator(byte: u8) -> bool {
    matches!(
        byte,
        b'\'' | b'"' | b'`' | b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')' | b'$' | b'\\'
    )
}

/// `MAX_MATCH_BYTES` skips only the candidate's window; scanning continues and
/// `limits_hit` records that the report may omit a secret. A later global stop
/// (`Work`, `Candidates`) overwrites the marker.
fn record_skipped_match(limits_hit: &mut Option<LimitExhausted>) {
    if limits_hit.is_none() {
        *limits_hit = Some(LimitExhausted::Match);
    }
}

fn evaluate_candidate(
    rules: &RuleSet,
    rule: &Rule,
    captures: &Captures<'_>,
    input: &str,
    work: &mut usize,
    limits: ScanLimits,
) -> Result<Option<Finding>, Abort> {
    let Some(spans) = candidate_spans(rule, captures, input)? else {
        return Ok(None);
    };
    evaluate_candidate_spans(rules, rule, spans, input, work, limits)
}

fn candidate_spans(
    rule: &Rule,
    captures: &Captures<'_>,
    input: &str,
) -> Result<Option<CandidateSpans>, Abort> {
    let full_match = captures.get(0).ok_or(ScanError::InvalidSpan)?;
    // A nonparticipating declared value, secret, or key group skips the candidate, rather than aborting the whole scan, reporting the whole match, or silently skipping the gate keyed on that group.
    let (selected, value_match) = if let Some(name) = rule.declaration.value_group.as_deref() {
        match captures.name(name) {
            Some(value) => (None, value),
            None => return Ok(None),
        }
    } else if let Some(group) = rule.declaration.secret_group {
        match captures.get(usize::from(group)) {
            Some(value) => (None, value),
            None => return Ok(None),
        }
    } else if let Some((index, alternative)) = first_unnamed_capture(rule, captures) {
        (Some(index), alternative)
    } else {
        (None, full_match)
    };
    if value_match.is_empty() {
        return Ok(None);
    }
    let gated = unquoted(rule, captures, value_match);
    let quoted = matches!(selected, Some(index) if rule.quoted_capture == Some(index));
    let reported = if quoted {
        (value_match.start() + 1, value_match.end() - 1)
    } else {
        (gated.start(), gated.end())
    };
    if reported.0 >= reported.1 {
        return Ok(None);
    }
    let key_match = match rule.declaration.key_group.as_deref() {
        Some(name) => match captures.name(name) {
            Some(key) => Some(key),
            None => return Ok(None),
        },
        None => None,
    };

    let full_span = TextSpan::snapped(input, full_match.start(), full_match.end())?;
    let value_span = TextSpan::snapped(input, gated.start(), gated.end())?;
    let reported_span = if quoted {
        TextSpan::snapped(input, reported.0, reported.1)?
    } else {
        value_span
    };
    let key_span = key_match
        .map(|value| TextSpan::snapped(input, value.start(), value.end()))
        .transpose()?;
    if !full_span.contains(value_span)
        || !value_span.contains(reported_span)
        || key_span.is_some_and(|span| !full_span.contains(span))
    {
        return Err(ScanError::InvalidSpan.into());
    }
    Ok(Some(CandidateSpans {
        full: full_span,
        value: value_span,
        reported: reported_span,
        key: key_span,
    }))
}

fn evaluate_candidate_spans(
    rules: &RuleSet,
    rule: &Rule,
    spans: CandidateSpans,
    input: &str,
    work: &mut usize,
    limits: ScanLimits,
) -> Result<Option<Finding>, Abort> {
    if spans.full.len() > MAX_MATCH_BYTES {
        return Err(Abort::Match);
    }
    let mut key_span = spans.key;
    let mut key_names_a_memorized_secret = rule.password_valued;
    if let Some(key) = spans.key {
        let key_bytes = input
            .as_bytes()
            .get(key.start()..key.end())
            .ok_or(ScanError::InvalidSpan)?;
        // `has_secret_key_token` reads every key byte, so charge its cost before rejecting the key.
        add_work(work, key_bytes.len(), limits.max_work_bytes)?;
        // Serialized text writes a line break as `\n`, whose letter the key pattern reads as the key's first character.
        let key = match key_after_escape(input, key)? {
            EscapedKey::Secret(after_escape) => after_escape,
            EscapedKey::Public => return Ok(None),
            EscapedKey::Absent if has_secret_key_token(key_bytes) => key,
            EscapedKey::Absent => return Ok(None),
        };
        let key_bytes = &input.as_bytes()[key.start()..key.end()];
        if rule.negated_switch_gate && is_negated_switch(key_bytes) {
            return Ok(None);
        }
        key_names_a_memorized_secret = ends_in_memorized_secret_word(key_bytes);
        key_span = Some(key);
    }
    // A detached value ends where serialized text writes its line's break.
    let spans = if rule.line_tail == Some(LineTail::Delimiter) {
        ended_at_escaped_line_break(input, spans)?
    } else {
        spans
    };
    // The continuation follows the key gate, so text after a key of another kind is left unread.
    let spans = if rule.continues_past_semicolons {
        continued_past_semicolons(input, spans, work, limits)?
    } else {
        spans
    };
    let CandidateSpans {
        full: full_span,
        value: value_span,
        reported,
        key: _,
    } = spans;
    if full_span.len() > MAX_MATCH_BYTES {
        return Err(Abort::Match);
    }
    let value = input
        .as_bytes()
        .get(value_span.start()..value_span.end())
        .ok_or(ScanError::InvalidSpan)?;
    let quoted = is_quoted(input.as_bytes(), value_span);
    if let Some(tail) = rule.line_tail
        && !quoted
        && !value_ends_its_line(input, reported.end(), tail, work, limits)?
    {
        return Ok(None);
    }
    // The shape gates read the whole value, continuation included: `12;a7B` is a password whose head is a number. The suppressors below read the text before the first `;`.
    let shaped = input
        .as_bytes()
        .get(value_span.start()..reported.end().max(value_span.end()))
        .ok_or(ScanError::InvalidSpan)?;
    if rule.declaration.reject_scalars
        && is_scalar(shaped)
        && !(key_names_a_memorized_secret && is_pin(shaped))
    {
        return Ok(None);
    }
    if rule.declaration.reject_placeholder_shapes && is_placeholder_shape(shaped) {
        return Ok(None);
    }
    if rule.structural_value_gate && is_structural_line(shaped) {
        return Ok(None);
    }
    if rule.port_mapping_gate && is_port_mapping(shaped) {
        return Ok(None);
    }
    // Under the overlay's option and field rules a quoted value is a literal, so only a bare
    // one reads as a member path or a type. The corpus gate reads quoted values as before.
    if rule.type_annotation_gate && !quoted && is_non_null_scalar_type(shaped) {
        return Ok(None);
    }
    if rule.code_reference_gate.is_some_and(|gate| {
        !(quoted && gate == CodeReferenceGate::Assignment) && names_code(gate, shaped)
    }) {
        return Ok(None);
    }

    let mut radius = rule.declaration.radius;
    if let Some(two_phase) = &rule.declaration.two_phase {
        let seed_start = full_span.start().saturating_sub(two_phase.seed_radius);
        let seed_end = full_span
            .end()
            .saturating_add(two_phase.seed_radius)
            .min(input.len());
        let seed = input
            .as_bytes()
            .get(seed_start..seed_end)
            .ok_or(ScanError::InvalidSpan)?;
        add_work(work, seed.len(), limits.max_work_bytes)?;
        let mut confirmed = false;
        for item in &two_phase.confirm_any {
            if contains_charged_ignore_case(seed, item.as_bytes(), work, limits)? {
                confirmed = true;
                break;
            }
        }
        if !confirmed {
            return Ok(None);
        }
        radius = radius.max(two_phase.full_radius);
    }

    let window_start = full_span.start().saturating_sub(radius);
    let window_end = full_span.end().saturating_add(radius).min(input.len());
    let window = input
        .as_bytes()
        .get(window_start..window_end)
        .ok_or(ScanError::InvalidSpan)?;
    add_work(work, window.len(), limits.max_work_bytes)?;

    if let Some(needle) = &rule.declaration.must_contain
        && !contains_charged_ignore_case(window, needle.as_bytes(), work, limits)?
    {
        return Ok(None);
    }
    if let Some(keywords) = &rule.declaration.keywords_any
        && !contains_any_charged_ignore_case(
            window,
            keywords,
            rule.keyword_matcher.as_ref(),
            work,
            limits,
        )?
    {
        return Ok(None);
    }
    if let Some(values) = &rule.declaration.value_suppressors_any
        && contains_any_charged_ignore_case(
            value,
            values,
            rule.suppressor_matcher.as_ref(),
            work,
            limits,
        )?
    {
        return Ok(None);
    }

    let char_class = rule.declaration.char_class.as_ref().or_else(|| {
        rule.declaration
            .entropy
            .as_ref()
            .and_then(|entropy| (entropy.min_bits_per_byte >= 3.0).then_some(&DEFAULT_CHAR_CLASS))
    });
    if char_class.is_some_and(|spec| {
        window.len() >= usize::from(spec.min_window_len)
            && lowercase_percent(window) > usize::from(spec.max_lower_pct)
    }) {
        return Ok(None);
    }

    let entropy_measured = if let Some(spec) = &rule.declaration.entropy {
        match entropy_allows(spec, value) {
            EntropyOutcome::Failed => return Ok(None),
            EntropyOutcome::Bypassed => false,
            EntropyOutcome::Passed => true,
        }
    } else {
        false
    };
    if (rule.source == RuleSource::Upstream || rule.declaration.upstream_parity)
        && (rules.context_is_safelisted(window)
            || rules.value_is_safelisted(value)
            || (!rule.declaration.uuid_format_secret && is_uuid(value)))
    {
        return Ok(None);
    }
    if rule
        .declaration
        .local_context
        .as_ref()
        .map(|spec| local_context_allows(spec, input.as_bytes(), value_span, work, limits))
        .transpose()?
        .is_some_and(|allowed| !allowed)
    {
        return Ok(None);
    }

    let offline = rule
        .declaration
        .offline_validation
        .as_ref()
        .map(|spec| offline_verdict(spec, value));
    if matches!(offline, Some(OfflineVerdict::Invalid)) {
        return Ok(None);
    }
    if confidence(rule, entropy_measured, offline) < minimum_confidence(rule, entropy_measured) {
        return Ok(None);
    }

    Ok(Some(Finding {
        rule_id: rule.declaration.name.clone(),
        rule_source: rule.source,
        full_span,
        value_span: reported,
        key_span,
    }))
}

fn confidence(rule: &Rule, entropy_measured: bool, offline: Option<OfflineVerdict>) -> i8 {
    let mut confidence = rule.confidence_bonus;
    if entropy_measured {
        confidence += 1;
    }
    if rule.declaration.keywords_any.is_some() {
        confidence += 2;
    }
    if matches!(offline, Some(OfflineVerdict::Valid)) {
        confidence += 5;
    }
    confidence
}

// The confidence threshold requires entropy only when measured; otherwise values below `entropy.min_len` bypass entropy and create an undeclared credential minimum.
fn minimum_confidence(rule: &Rule, entropy_measured: bool) -> i8 {
    rule.declaration.min_confidence.unwrap_or({
        if rule.declaration.keywords_any.is_some() && entropy_measured {
            3
        } else if rule.declaration.keywords_any.is_some() {
            2
        } else {
            0
        }
    })
}

/// Parses the ASCII subset of one keyed overlay rule without regex matching.
///
/// Each variant mirrors one embedded `magic-keyed-*` regex. For ASCII input
/// `probe` must report exactly the candidates `Regex::captures_iter` reports
/// for that regex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyedForm {
    /// `key = value`, the value ending at a separator or shell delimiter.
    Assignment,
    /// `key = "value"`.
    AssignmentQuoted { value_quote: u8 },
    /// `"key": "value"`.
    QuotedKeyed { key_quote: u8, value_quote: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Probe {
    Candidate(CandidateSpans),
    /// No candidate is anchored in `at..next`; the next probe starts at `next`.
    Skip(usize),
}

impl KeyedForm {
    fn for_rule(name: &str) -> Option<Self> {
        match name {
            "magic-keyed-assignment" => Some(Self::Assignment),
            "magic-keyed-assignment-double-quoted" => {
                Some(Self::AssignmentQuoted { value_quote: b'"' })
            }
            "magic-keyed-assignment-single-quoted" => {
                Some(Self::AssignmentQuoted { value_quote: b'\'' })
            }
            "magic-keyed-double-quoted" => Some(Self::QuotedKeyed {
                key_quote: b'"',
                value_quote: b'"',
            }),
            "magic-keyed-single-quoted" => Some(Self::QuotedKeyed {
                key_quote: b'\'',
                value_quote: b'\'',
            }),
            "magic-keyed-double-single" => Some(Self::QuotedKeyed {
                key_quote: b'"',
                value_quote: b'\'',
            }),
            "magic-keyed-single-double" => Some(Self::QuotedKeyed {
                key_quote: b'\'',
                value_quote: b'"',
            }),
            _ => None,
        }
    }

    /// The byte every candidate of this form contains.
    fn anchor(self) -> u8 {
        match self {
            Self::Assignment | Self::AssignmentQuoted { .. } => b'=',
            Self::QuotedKeyed { key_quote, .. } => key_quote,
        }
    }

    /// Probes for the candidate anchored at `at`.
    ///
    /// `floor` is where `captures_iter` would resume after the previous
    /// candidate, so no reported span starts before it.
    fn probe(self, input: &str, at: usize, floor: usize) -> Result<Probe, ScanError> {
        let bytes = input.as_bytes();
        match self {
            Self::Assignment => {
                let Some((key_start, key_end)) = assignment_key(bytes, at, floor) else {
                    return Ok(Probe::Skip(at + 1));
                };
                // Spaces follow `=` on its line only when a separator precedes it (`key = value`); a value detached from `=` any other way belongs to `magic-detached-assignment`.
                let value_start = if key_end < at {
                    skip_inline_spaces(bytes, at + 1)
                } else {
                    at + 1
                };
                // A second `=` makes the operator a comparison.
                if bytes.get(value_start) == Some(&b'=') {
                    return Ok(Probe::Skip(at + 1));
                }
                let value_end = unquoted_value_end(bytes, value_start);
                if value_start == value_end {
                    return Ok(Probe::Skip(at + 1));
                }
                candidate(
                    input,
                    (key_start, value_end),
                    (value_start, value_end),
                    (key_start, key_end),
                )
            }
            Self::AssignmentQuoted { value_quote } => {
                let Some((key_start, key_end)) = assignment_key(bytes, at, floor) else {
                    return Ok(Probe::Skip(at + 1));
                };
                let value_quote_start = skip_rule_spaces(bytes, at + 1);
                if bytes.get(value_quote_start) != Some(&value_quote) {
                    return Ok(Probe::Skip(at + 1));
                }
                let value_start = value_quote_start + 1;
                match quoted_end(bytes, value_start, value_quote) {
                    Ok(value_end) => candidate(
                        input,
                        (key_start, value_end + 1),
                        (value_start, value_end),
                        (key_start, key_end),
                    ),
                    // An `=` inside this unclosed value opens its own value at a quote this walk consumed as escaped, so that value cannot close before `resume` either.
                    Err(resume) => Ok(Probe::Skip(resume)),
                }
            }
            Self::QuotedKeyed {
                key_quote,
                value_quote,
            } => {
                let key_start = at + 1;
                let key_end = match quoted_end(bytes, key_start, key_quote) {
                    Ok(end) => end,
                    // A key opening at an escaped quote inside this one is parsed in step with it, so it cannot close before `resume` either.
                    Err(resume) => return Ok(Probe::Skip(resume)),
                };
                // A key opening at an escaped quote inside this one is a suffix of this key that shares its closing quote, so it fails the same checks; the closing quote itself may open the next key.
                let skip = Probe::Skip(key_end);
                if !quoted_key_contains_keyword(&bytes[key_start..key_end]) {
                    return Ok(skip);
                }
                let separator = skip_rule_spaces(bytes, key_end + 1);
                if bytes.get(separator) != Some(&b':') {
                    return Ok(skip);
                }
                let value_quote_start = skip_rule_spaces(bytes, separator + 1);
                if bytes.get(value_quote_start) != Some(&value_quote) {
                    return Ok(skip);
                }
                let value_start = value_quote_start + 1;
                let Ok(value_end) = quoted_end(bytes, value_start, value_quote) else {
                    return Ok(skip);
                };
                candidate(
                    input,
                    (at, value_end + 1),
                    (value_start, value_end),
                    (key_start, key_end),
                )
            }
        }
    }
}

fn candidate(
    input: &str,
    (full_start, full_end): (usize, usize),
    (value_start, value_end): (usize, usize),
    (key_start, key_end): (usize, usize),
) -> Result<Probe, ScanError> {
    let value = TextSpan::snapped(input, value_start, value_end)?;
    Ok(Probe::Candidate(CandidateSpans {
        full: TextSpan::snapped(input, full_start, full_end)?,
        value,
        reported: value,
        key: Some(TextSpan::snapped(input, key_start, key_end)?),
    }))
}

/// Finds the key that `(?-u:\b)(?P<key>[A-Za-z0-9_.-]*(?:keyword)[A-Za-z0-9_.-]*)[sep]*=`
/// reports before the `=` at `equals` when the search starts at `floor`.
fn assignment_key(bytes: &[u8], equals: usize, floor: usize) -> Option<(usize, usize)> {
    let mut key_end = equals;
    while key_end > floor && is_rule_space_byte(bytes[key_end - 1]) {
        key_end -= 1;
    }
    let mut run_start = key_end;
    while run_start > floor && is_key_byte(bytes[run_start - 1]) {
        run_start -= 1;
    }
    // `\b` holds at a word byte only when the byte before it is not one; the byte before `floor` counts even though the search does not start there.
    let key_start = (run_start..key_end).find(|&index| {
        is_word_byte(bytes[index]) && (index == 0 || !is_word_byte(bytes[index - 1]))
    })?;
    contains_keyed_keyword(&bytes[key_start..key_end]).then_some((key_start, key_end))
}

fn skip_rule_spaces(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).copied().is_some_and(is_rule_space_byte) {
        index += 1;
    }
    index
}

fn skip_inline_spaces(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).copied().is_some_and(is_inline_space_byte) {
        index += 1;
    }
    index
}

/// Extends `(?:[^SEP'"`;&|<>()$\\]|\\.)+` from `start` and returns where it stops.
fn unquoted_value_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while let Some(&byte) = bytes.get(end) {
        match byte {
            // `.` never matches a newline, so a backslash before one, or at the end of input, escapes nothing and ends the value.
            b'\\' => match bytes.get(end + 1) {
                None | Some(b'\n') => break,
                Some(_) => end += 2,
            },
            b'\'' | b'"' | b'`' | b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')' | b'$' => break,
            _ if is_rule_space_byte(byte) => break,
            _ => end += 1,
        }
    }
    end
}

/// Finds the unescaped `quote` that closes `(?:[^QUOTE\\]|\\.)*` starting at `start`.
///
/// `Err(resume)` means no closing quote exists, and no section opening at a
/// quote inside `start..resume` closes either: those quotes are escaped, so a
/// section opening there is parsed in step with this one and stops where it
/// stopped. `.` never matches a newline, so a backslash before one, or at the
/// end of input, ends the section without closing it.
fn quoted_end(bytes: &[u8], start: usize, quote: u8) -> Result<usize, usize> {
    let mut cursor = start;
    while let Some(&byte) = bytes.get(cursor) {
        if byte == b'\\' {
            match bytes.get(cursor + 1) {
                None => return Err(bytes.len()),
                Some(b'\n') => return Err(cursor + 2),
                Some(_) => cursor += 2,
            }
        } else if byte == quote {
            return Ok(cursor);
        } else {
            cursor += 1;
        }
    }
    Err(bytes.len())
}

// The keyed overlay rules spell their separator and value-terminator class as `[\t\n\x0B\f\r ...]`, and `u8::is_ascii_whitespace` omits `\x0B`.
// These helpers run only for ASCII input, so the non-ASCII spellings in that class are unreachable here.
fn is_rule_space_byte(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | 0x0B | b'\x0C' | b'\r' | b' ')
}

// The unquoted assignment rule reads the separator class less the line feed and the carriage return between `=` and a value on the line of the `=`.
fn is_inline_space_byte(byte: u8) -> bool {
    matches!(byte, b'\t' | 0x0B | b'\x0C' | b' ')
}

/// The separator class the keyed overlay rules spell out, for text of any encoding.
fn is_rule_space(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\x0B' | '\x0C' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// Extends `(?:[^SEP'"`;&|<>()$\\]|\\.)*` from `start` over text of any encoding and returns where it stops.
fn unquoted_run_end(input: &str, start: usize) -> usize {
    let mut characters = input[start..].char_indices().peekable();
    while let Some((offset, character)) = characters.next() {
        match character {
            '\\' => match characters.peek() {
                None | Some((_, '\n')) => return start + offset,
                Some(_) => {
                    characters.next();
                }
            },
            '\'' | '"' | '`' | ';' | '&' | '|' | '<' | '>' | '(' | ')' | '$' => {
                return start + offset;
            }
            _ if is_rule_space(character) => return start + offset,
            _ => {}
        }
    }
    input.len()
}

/// Longest text after a value that the `;` continuation reads.
///
/// The bound is far below the margin a scan window keeps around a finding, so the walk reads
/// the same bytes whichever window holds the value. A tail still open at the bound fails the
/// scan closed.
const MAX_CONTINUATION_BYTES: usize = MAX_LOCAL_CONTEXT_BYTES;

/// Extends the reported span of an unquoted assignment value through `;` to the end of its line.
///
/// `;` ends a shell statement and separates connection-string fields, and it also occurs inside
/// an unquoted credential (`PASSWORD=ab;cd` in an environment file). The value continues when
/// the text joined by `;` runs to the end of the line, apart from trailing blanks, and is free
/// of `=`: a `name=value` pair after `;` is the next field, and a word followed by more text is
/// the next statement. Punctuation after the joined text closes a statement and stays outside
/// the span (`ab;cd;`, `ab;cd;}`). A line ends at a line break, at the end of the input, or at
/// a line break written as the escape `\n` or `\r`, as serialized text carries it.
fn continued_past_semicolons(
    input: &str,
    spans: CandidateSpans,
    work: &mut usize,
    limits: ScanLimits,
) -> Result<CandidateSpans, Abort> {
    let bytes = input.as_bytes();
    let start = spans.reported.end();
    if start != spans.full.end() || bytes.get(start) != Some(&b';') {
        return Ok(spans);
    }
    let window_end = input.ceil_char_boundary(start.saturating_add(MAX_CONTINUATION_BYTES));
    add_work(work, window_end - start, limits.max_work_bytes)?;
    let window = &input[..window_end];
    // `end` is where the joined value text ends; `closed` is where the walk stands, past any
    // closing punctuation after that text (`ab;cd;`, `ab;cd;}`).
    let mut end = start;
    let mut closed = start;
    while bytes.get(closed) == Some(&b';') && closed < window_end {
        let run_end = unquoted_run_end(window, closed + 1);
        let segment_end =
            escaped_line_break(&bytes[closed + 1..run_end]).map_or(run_end, |at| closed + 1 + at);
        let segment = &bytes[closed + 1..segment_end];
        // A pair is the next field, and value text ends at the punctuation that closes a statement.
        if segment.contains(&b'=') || (end < closed && !is_punctuation(segment)) {
            return Ok(spans);
        }
        if !is_punctuation(segment) {
            end = segment_end;
        }
        closed = segment_end;
    }
    let cut_escape = closed + 1 == window_end && bytes[closed] == b'\\';
    if (closed == window_end || cut_escape) && window_end < input.len() {
        // The tail is still open at the bound, so the value's extent is unknown and the scan fails closed.
        return Err(Abort::Match);
    }
    if end == start {
        return Ok(spans);
    }
    let blanks_end = closed
        + bytes[closed..window_end]
            .iter()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
    let line_ends = match &bytes[blanks_end..window_end] {
        // The end of the input, or blanks that reach the bound of the walk.
        [] => true,
        [b'\n' | b'\r', ..] | [b'\\', b'n' | b'r' | b'u', ..] => true,
        [b'#', ..] => blanks_end > closed,
        _ => false,
    };
    if !line_ends {
        return Ok(spans);
    }
    Ok(CandidateSpans {
        full: TextSpan::snapped(input, spans.full.start(), end)?,
        value: spans.value,
        reported: TextSpan::snapped(input, spans.reported.start(), end)?,
        key: spans.key,
    })
}

/// Whether the text after an unquoted value lets the line gate report it.
///
/// The value is reported when its line ends after it: at a line break, at the end of the
/// input, at a `#` comment after blanks, or at a backslash, which continues a shell line and
/// opens the escape serialized text writes for a line break. A delimiter directly after the
/// value also ends it, as [`LineTail`] names per rule. Blanks and then another word mean the
/// value position holds the first word of other text (`KEY= command argument`).
///
/// The gate reads at most [`MAX_CONTINUATION_BYTES`] past the value and charges them; a run of
/// blanks that long counts as the end of the line.
fn value_ends_its_line(
    input: &str,
    end: usize,
    tail: LineTail,
    work: &mut usize,
    limits: ScanLimits,
) -> Result<bool, Abort> {
    let window_end = input.ceil_char_boundary(end.saturating_add(MAX_CONTINUATION_BYTES));
    let rest = input.get(end..window_end).ok_or(ScanError::InvalidSpan)?;
    add_work(work, rest.len(), limits.max_work_bytes)?;
    let after_blanks = rest.trim_start_matches(is_inline_rule_space);
    let attached = after_blanks.len() == rest.len();
    let delimited = tail == LineTail::Delimiter;
    if after_blanks.starts_with(is_line_break) {
        return Ok(true);
    }
    Ok(match after_blanks.as_bytes() {
        [] => true,
        [b'\\', after @ ..] => match after {
            // A shell line continues, or serialized text writes its line break.
            [] | [b'\n' | b'\r' | b'n' | b'r', ..] => true,
            [b'u', b'0', b'0', b'0', b'a' | b'd', ..] => true,
            // Serialized text writes the backslash of a continued shell line as a pair.
            [b'\\', b'\\', b'n' | b'r', ..] => true,
            // Serialized text escapes the quote that closes the string.
            [b'"' | b'\'', ..] => closes_a_string(&after_blanks[2..]),
            _ => false,
        },
        [b'#', ..] => !attached,
        [b'"' | b'\'', ..] => {
            (attached && delimited)
                || ((attached || delimited) && closes_a_string(&after_blanks[1..]))
        }
        [
            b'`' | b')' | b']' | b'}' | b'<' | b'>' | b';' | b'|' | b'&',
            ..,
        ] => delimited,
        _ => attached && delimited,
    })
}

/// Whether a quote closes the string around a value: up to the next blank, backslash, or
/// end, the text after it is closing punctuation (`"`, `")`, `"},`). A letter or a digit
/// there puts the quote inside a word (`Pa55'word9`, `ab'.cd9`).
fn closes_a_string(after_quote: &str) -> bool {
    let word_end = after_quote
        .find(|character: char| is_rule_space(character) || character == '\\')
        .unwrap_or(after_quote.len());
    after_quote.as_bytes()[..word_end]
        .iter()
        .all(u8::is_ascii_punctuation)
}

/// A break between lines: the line feed and carriage return, the vertical tab and form feed,
/// and the Unicode line and paragraph separators.
fn is_line_break(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r' | '\x0B' | '\x0C' | '\u{2028}' | '\u{2029}'
    )
}

/// The separator class less the line feed and the carriage return, for text of any encoding.
fn is_inline_rule_space(character: char) -> bool {
    is_rule_space(character) && !is_line_break(character)
}

/// Ends a detached value at the first line break its run writes as an escape.
///
/// Serialized text carries a line break as `\n`, so the run of `KEY= value\nnext` holds the
/// first word of the next line. An escape that opens the run stays in it: the unquoted
/// assignment rule reads `KEY=\nvalue` the same way.
fn ended_at_escaped_line_break(
    input: &str,
    spans: CandidateSpans,
) -> Result<CandidateSpans, ScanError> {
    if spans.reported != spans.value || spans.full.end() != spans.value.end() {
        return Ok(spans);
    }
    let run = &input.as_bytes()[spans.value.start()..spans.value.end()];
    let mut lead = 0;
    while escaped_line_break(&run[lead..]) == Some(0) {
        lead += if run[lead + 1] == b'u' { 6 } else { 2 };
    }
    let Some(at) = escaped_line_break(&run[lead..]) else {
        return Ok(spans);
    };
    let end = spans.value.start() + lead + at;
    let value = TextSpan::new(input, spans.value.start(), end)?;
    Ok(CandidateSpans {
        full: TextSpan::new(input, spans.full.start(), end)?,
        value,
        reported: value,
        key: spans.key,
    })
}

/// Where a run of value text first writes a line break as the escape `\n` or `\r`.
fn escaped_line_break(run: &[u8]) -> Option<usize> {
    let mut index = 0;
    while index < run.len() {
        if run[index] != b'\\' {
            index += 1;
        } else if matches!(run.get(index + 1), Some(b'n' | b'r'))
            || run[index + 1..].starts_with(b"u000a")
            || run[index + 1..].starts_with(b"u000d")
        {
            return Some(index);
        } else {
            index += 2;
        }
    }
    None
}

/// What follows the escape that a key pattern read as the start of its key.
enum EscapedKey {
    /// The name after the escape passes the key gate.
    Secret(TextSpan),
    /// The name after the escape carries a public marker, as `\npublic_key` does.
    Public,
    /// The key follows no escape, or the name after it is of another kind.
    Absent,
}

/// Reads the key that begins after an escaped line break or tab: the letter of `\n`, `\r`,
/// or `\t`, or the `u000a`, `u000d`, or `u0009` of the form canonical JSON writes.
fn key_after_escape(input: &str, key: TextSpan) -> Result<EscapedKey, ScanError> {
    let bytes = input.as_bytes();
    // An odd run of backslashes ends in one that escapes the key's first letter; an even run
    // is escaped backslashes (`C:\\\\name`). A run that reaches the bound reads as no escape.
    let backslashes = bytes[..key.start()]
        .iter()
        .rev()
        .take(MAX_ESCAPE_RUN_BYTES)
        .take_while(|byte| **byte == b'\\')
        .count();
    if backslashes % 2 == 0 || backslashes == MAX_ESCAPE_RUN_BYTES {
        return Ok(EscapedKey::Absent);
    }
    let name = &bytes[key.start()..key.end()];
    let (escape_len, line_break) = match name {
        [b'u', b'0', b'0', b'0', b'a' | b'A' | b'd' | b'D', ..] => (5, true),
        [b'u', b'0', b'0', b'0', b'9', ..] => (5, false),
        [b'n' | b'r', ..] => (1, true),
        [b't', ..] => (1, false),
        _ => return Ok(EscapedKey::Absent),
    };
    let after = &name[escape_len..];
    // The public marker applies after a line break, where the name starts a line as
    // `public_key=` does; after a tab the key is read as written.
    if key_tokens(after).any(|token| {
        NON_SECRET_KEY_MARKERS
            .iter()
            .any(|marker| token.eq_ignore_ascii_case(marker))
    }) {
        return Ok(if line_break {
            EscapedKey::Public
        } else {
            EscapedKey::Absent
        });
    }
    if !has_secret_key_token(after) {
        return Ok(EscapedKey::Absent);
    }
    TextSpan::new(input, key.start() + escape_len, key.end()).map(EscapedKey::Secret)
}

/// Longest run of backslashes read before a key to tell an escape from an escaped backslash.
const MAX_ESCAPE_RUN_BYTES: usize = 64;

/// `--no-password` switches the prompt off and takes no value, so the word after it is the
/// next argument.
fn is_negated_switch(key: &[u8]) -> bool {
    key_tokens(key)
        .next()
        .is_some_and(|token| token.eq_ignore_ascii_case(b"no"))
}

/// A line of structure where a detached value would stand: a closing bracket or a rule of
/// punctuation, a bracketed section header, a comment, or the next assignment.
fn is_structural_line(value: &[u8]) -> bool {
    let value = value.trim_ascii();
    let (Some(&first), Some(&last)) = (value.first(), value.last()) else {
        return true;
    };
    is_punctuation(value)
        || first == b'#'
        || matches!((first, last), (b'<', b'>') | (b'{', b'}') | (b'[', b']'))
        || is_assignment(value)
}

/// `DATABASE_URL=postgres://db/app`, `URL=`: a name, one `=`, and the text assigned to it.
///
/// Base64 padding also ends a value in `=`. A doubled `=` is padding, and so is a single `=`
/// after a body that mixes letter cases; a name in one letter case with one `=` reads as the
/// empty assignment.
fn is_assignment(value: &[u8]) -> bool {
    let name_end = value
        .iter()
        .position(|byte| !is_key_byte(*byte))
        .unwrap_or(value.len());
    let name = &value[..name_end];
    if !name
        .first()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        || value.get(name_end) != Some(&b'=')
    {
        return false;
    }
    match value.get(name_end + 1) {
        // A `;` joins further text to the word, which leaves the word itself ending in `=`.
        None | Some(b';') => is_single_case_name(name),
        Some(next) => *next != b'=',
    }
}

fn is_punctuation(value: &[u8]) -> bool {
    !value
        .iter()
        .any(|byte| byte.is_ascii_alphanumeric() || !byte.is_ascii())
}

/// `URL`, `api_key`, `db.password`, `x-api-key`: letters of one case, digits, and the
/// punctuation of a key name.
fn is_single_case_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.iter().copied().all(is_key_byte)
        && !(name.iter().any(u8::is_ascii_uppercase) && name.iter().any(u8::is_ascii_lowercase))
}

/// Whether the same quote character stands directly before and after the value.
fn is_quoted(bytes: &[u8], value: TextSpan) -> bool {
    let before = value.start().checked_sub(1).and_then(|at| bytes.get(at));
    matches!(before, Some(b'"' | b'\'')) && before == bytes.get(value.end())
}

fn is_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

const KEYED_KEYWORDS: [&str; 7] = [
    "auth",
    "bearer",
    "credential",
    "key",
    "password",
    "secret",
    "token",
];

/// Case-insensitive automaton over `KEYED_KEYWORDS`. Its prefilter avoids
/// scanning keyword-free quoted spans byte by byte.
static KEYED_KEYWORD_AUTOMATON: LazyLock<AhoCorasick> = LazyLock::new(|| {
    AhoCorasickBuilder::new()
        .match_kind(MatchKind::LeftmostFirst)
        .ascii_case_insensitive(true)
        .kind(Some(AhoCorasickKind::DFA))
        .build(KEYED_KEYWORDS)
        .expect("keyed keyword automaton")
});

fn contains_keyed_keyword(key: &[u8]) -> bool {
    KEYED_KEYWORD_AUTOMATON.is_match(key)
}

// `(?:[^QUOTE\\]|\\.)*` parses left to right into single bytes and `\x` pairs, so the keyword alternation can only start on one of those unit boundaries.
fn quoted_key_contains_keyword(key: &[u8]) -> bool {
    if !KEYED_KEYWORD_AUTOMATON.is_match(key) {
        return false;
    }
    // Without a backslash every byte starts a unit, so any occurrence qualifies.
    if memchr::memchr(b'\\', key).is_none() {
        return true;
    }
    let mut index = 0;
    while index < key.len() {
        if key[index] == b'\\' {
            index += 2;
            continue;
        }
        if keyed_keyword_at(key, index) {
            return true;
        }
        index += 1;
    }
    false
}

fn keyed_keyword_at(key: &[u8], index: usize) -> bool {
    let rest = &key[index..];
    KEYED_KEYWORDS.iter().any(|keyword| {
        rest.get(..keyword.len())
            .is_some_and(|window| window.eq_ignore_ascii_case(keyword.as_bytes()))
    })
}

// Unnamed captures hold secrets; named captures mark header fields. Ignore named captures so header-only rules return their whole match.
// Fallback rules treat an unnamed capture spanning the quoted interior as the secret; declared value and secret groups take precedence.
fn unquoted<'a>(
    rule: &Rule,
    captures: &Captures<'a>,
    selected: regex::bytes::Match<'a>,
) -> regex::bytes::Match<'a> {
    if rule.declaration.value_group.is_some() || rule.declaration.secret_group.is_some() {
        return selected;
    }
    let bytes = selected.as_bytes();
    let quoted = bytes.len() >= 2
        && matches!(bytes[0], b'\'' | b'"' | b'`')
        && bytes[bytes.len() - 1] == bytes[0];
    if !quoted {
        return selected;
    }
    rule.unnamed_captures
        .iter()
        .filter_map(|&index| captures.get(index))
        .find(|inner| inner.start() == selected.start() + 1 && inner.end() == selected.end() - 1)
        .unwrap_or(selected)
}

fn first_unnamed_capture<'a>(
    rule: &Rule,
    captures: &Captures<'a>,
) -> Option<(usize, regex::bytes::Match<'a>)> {
    rule.unnamed_captures
        .iter()
        .filter_map(|&index| Some((index, captures.get(index)?)))
        .find(|(_, value)| !value.is_empty())
}

fn add_work(total: &mut usize, amount: usize, limit: usize) -> Result<(), Abort> {
    *total = total.checked_add(amount).ok_or(Abort::Work)?;
    if *total > limit {
        return Err(Abort::Work);
    }
    Ok(())
}

// Corpus and overlay lists spell each keyword and suppressor in lowercase and
// uppercase by hand, so a case-sensitive search misses the mixed-case spelling
// of both: it drops a secret keyed `ApiKey` and reports a value of `ChangeMe`.
fn contains_charged_ignore_case(
    haystack: &[u8],
    needle: &[u8],
    work: &mut usize,
    limits: ScanLimits,
) -> Result<bool, Abort> {
    add_work(work, haystack.len(), limits.max_work_bytes)?;
    Ok(find_ignore_ascii_case(haystack, needle))
}

fn contains_any_charged_ignore_case(
    haystack: &[u8],
    needles: &[String],
    matcher: Option<&AhoCorasick>,
    work: &mut usize,
    limits: ScanLimits,
) -> Result<bool, Abort> {
    let Some(matcher) = matcher else {
        return contains_any_charged_ignore_case_scalar(haystack, needles, work, limits);
    };
    let Some(full_charge) = haystack.len().checked_mul(needles.len()) else {
        return contains_any_charged_ignore_case_scalar(haystack, needles, work, limits);
    };
    if work
        .checked_add(full_charge)
        .is_none_or(|total| total > limits.max_work_bytes)
    {
        return contains_any_charged_ignore_case_scalar(haystack, needles, work, limits);
    }

    // Pattern 0 is the lowest possible index, so no later overlap can change `first_match`.
    let mut first_match: Option<usize> = None;
    for found in matcher.find_overlapping_iter(haystack) {
        let index = found.pattern().as_usize();
        if first_match.is_none_or(|lowest| index < lowest) {
            first_match = Some(index);
        }
        if index == 0 {
            break;
        }
    }
    let probes = first_match.map_or(needles.len(), |index| index + 1);
    *work += haystack.len() * probes;
    Ok(first_match.is_some())
}

fn contains_any_charged_ignore_case_scalar(
    haystack: &[u8],
    needles: &[String],
    work: &mut usize,
    limits: ScanLimits,
) -> Result<bool, Abort> {
    for needle in needles {
        if contains_charged_ignore_case(haystack, needle.as_bytes(), work, limits)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn find_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    let lower = needle[0].to_ascii_lowercase();
    let upper = needle[0].to_ascii_uppercase();
    let limit = haystack.len() - needle.len() + 1;
    let mut offset = 0;
    while offset < limit {
        let found = if lower == upper {
            memchr::memchr(lower, &haystack[offset..limit])
        } else {
            memchr::memchr2(lower, upper, &haystack[offset..limit])
        };
        let Some(position) = found else {
            return false;
        };
        let start = offset + position;
        if haystack[start..start + needle.len()].eq_ignore_ascii_case(needle) {
            return true;
        }
        offset = start + 1;
    }
    false
}

pub(crate) const SECRET_KEY_WORDS: &[&str] = &[
    "key",
    "keys",
    "token",
    "tokens",
    "secret",
    "secrets",
    "password",
    "passwords",
    "auth",
    "authorization",
    "bearer",
    "credential",
    "credentials",
];

// KEY_QUALIFIERS limits undelimited matches around a secret word: `monkey` and `whiskey` do not match because `mon` and `whis` are not qualifiers, and `keyboard` and `keywords` do not because `board` and `words` are not. `public` is excluded because public keys are not credentials.
const KEY_QUALIFIERS: &[&[u8]] = &[
    b"access",
    b"admin",
    b"api",
    b"app",
    b"application",
    b"auth",
    b"aws",
    b"azure",
    b"bearer",
    b"client",
    b"consumer",
    b"database",
    b"db",
    b"encryption",
    b"gcp",
    b"github",
    b"gitlab",
    b"hmac",
    b"jwt",
    b"master",
    b"oauth",
    b"private",
    b"refresh",
    b"root",
    b"secret",
    b"service",
    b"session",
    b"shared",
    b"signing",
    b"slack",
    b"stripe",
    b"twilio",
    b"user",
    b"vault",
    b"webhook",
    b"b64",
    b"base64",
    b"data",
    b"env",
    b"file",
    b"hash",
    b"hashes",
    b"hex",
    b"id",
    b"ids",
    b"name",
    b"names",
    b"path",
    b"plain",
    b"prefix",
    b"ref",
    b"string",
    b"text",
    b"value",
    b"values",
];

// A key naming the public half of a keypair is not a credential, and the marker
// has to apply across delimited tokens too: `public_key` splits into `public` and
// `key`, so excluding `public` from the qualifiers only covers `publickey`.
const NON_SECRET_KEY_MARKERS: &[&[u8]] = &[b"public", b"pubkey", b"publishable"];

fn has_secret_key_token(key: &[u8]) -> bool {
    if key_tokens(key).any(|token| {
        NON_SECRET_KEY_MARKERS
            .iter()
            .any(|marker| token.eq_ignore_ascii_case(marker))
    }) {
        return false;
    }
    key_tokens(key).any(|token| {
        token_matches_any_key_name(token, SECRET_KEY_WORDS) || ends_with_password(token)
    })
}

/// A token that ends in `password` names one whatever precedes it: `PGPASSWORD`,
/// `keystorepassword`. The other key words need a qualifier chain in front, because they end
/// ordinary words (`monkey`, `turkey`).
const PASSWORD_SUFFIX: &[u8] = b"password";

fn ends_with_password(token: &[u8]) -> bool {
    token
        .len()
        .checked_sub(PASSWORD_SUFFIX.len())
        .is_some_and(|start| token[start..].eq_ignore_ascii_case(PASSWORD_SUFFIX))
}

/// Key words whose value a person chooses or memorizes, so it can be all digits.
/// A number under `token` or `key` is a count or an index, as in `max_tokens=4096`.
const MEMORIZED_SECRET_WORDS: &[&[u8]] = &[b"secret"];

/// Whether the last word of `key` is `secret` or ends in `password`.
/// `password_length` and `secret_version` end in a word that names a number.
fn ends_in_memorized_secret_word(key: &[u8]) -> bool {
    key_tokens(key).last().is_some_and(|token| {
        ends_with_password(token)
            || MEMORIZED_SECRET_WORDS
                .iter()
                .any(|word| token.eq_ignore_ascii_case(word))
    })
}

/// The shortest personal identification number in common use has four digits (ISO 9564-1).
/// Shorter numbers under a password key are lengths, counts, and flags.
const MIN_PIN_DIGITS: usize = 4;

fn is_pin(value: &[u8]) -> bool {
    let value = value.trim_ascii();
    value.len() >= MIN_PIN_DIGITS && value.iter().all(u8::is_ascii_digit)
}

/// Whether `value` has the shape of text that stands where a credential would go.
///
/// - A plain word, which a metavariable (`PASSWORD`), a type name (`string`), and a word of
///   prose all are; a generated credential carries an ASCII digit or symbol.
/// - A mask: a run of `***`, or punctuation alone (`...`).
/// - A reference: a leading `$`, or a trailing upper-case name that itself names a secret
///   (`os.environ/DB_PASSWORD`).
/// - A template: wrapped in `<>`, `{}`, or `[]`, `%` and one letter, or `%NAME%`.
/// - A port followed by a property list (`1433;database=sales`).
/// - A file path or a URL, which names where the credential is kept or issued.
/// - An attribute list (`id=npm,src=.npmrc`), which describes the secret an option names.
fn is_placeholder_shape(value: &[u8]) -> bool {
    let value = value.trim_ascii();
    let (Some(&first), Some(&last)) = (value.first(), value.last()) else {
        return true;
    };
    is_plain_word(value)
        || is_mask(value)
        || first == b'$'
        || names_a_secret_variable(value)
        || matches!((first, last), (b'<', b'>') | (b'{', b'}') | (b'[', b']'))
        || matches!(value, [b'%', verb] if verb.is_ascii_alphabetic())
        || is_environment_reference(value)
        || is_file_path(value)
        || is_url(value)
        || is_attribute(value)
        || is_port_and_properties(value)
}

/// `%DB_PASSWORD%`: a variable name between two `%`, as the Windows shell writes a reference.
fn is_environment_reference(value: &[u8]) -> bool {
    value
        .strip_prefix(b"%")
        .and_then(|rest| rest.strip_suffix(b"%"))
        .is_some_and(|name| {
            name.first()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
                && name
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        })
}

/// `1433;database=sales`: a port and the property list a JDBC URL writes after it.
fn is_port_and_properties(value: &[u8]) -> bool {
    let digits = value
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    digits > 0 && value.get(digits) == Some(&b';')
}

/// `8080:80`, `127.0.0.1:5432:5432`: numbers joined by `:` are a port mapping, which `-p`
/// takes under `docker run` and `ssh`.
fn is_port_mapping(value: &[u8]) -> bool {
    value.contains(&b':')
        && value
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b':' | b'.' | b'/'))
}

/// The built-in scalar types of the GraphQL specification in their non-null spelling.
const NON_NULL_SCALAR_TYPES: &[&str] = &["String!", "ID!", "Int!", "Float!", "Boolean!"];

/// `password: String!` in a schema declares a field of a built-in scalar type.
fn is_non_null_scalar_type(value: &[u8]) -> bool {
    NON_NULL_SCALAR_TYPES
        .iter()
        .any(|name| value == name.as_bytes())
}

/// `token: https://example.org/settings/tokens` names where a credential is issued.
/// A URL that carries a password in its userinfo is a finding of its own rule.
fn is_url(value: &[u8]) -> bool {
    let scheme_end = value
        .iter()
        .position(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-')))
        .unwrap_or(value.len());
    value.first().is_some_and(u8::is_ascii_alphabetic) && value[scheme_end..].starts_with(b"://")
}

/// `id=npm,src=.npmrc`: a lower-case name, `=`, and more text is an attribute of the thing the
/// option names, as in `docker build --secret id=npm`. Base64 padding ends its value or doubles
/// the `=`, and its body mixes case.
fn is_attribute(value: &[u8]) -> bool {
    let name_end = value
        .iter()
        .position(|byte| !(byte.is_ascii_lowercase() || *byte == b'_'))
        .unwrap_or(value.len());
    name_end > 0
        && value.get(name_end) == Some(&b'=')
        && value.get(name_end + 1).is_some_and(|byte| *byte != b'=')
}

/// Letters of any script, `_`, `-`, and whitespace, before the closing punctuation a synopsis
/// or a sentence adds (`[--password PASSWORD]`, `the --token value.`). A line break or tab
/// written as an escape (`\n`) is whitespace.
fn is_plain_word(value: &[u8]) -> bool {
    let word_end = value
        .iter()
        .rposition(|byte| !matches!(byte, b']' | b')' | b'}' | b',' | b'.' | b':'))
        .map_or(0, |index| index + 1);
    let word = &value[..word_end];
    let mut index = 0;
    while let Some(&byte) = word.get(index) {
        let escaped_space =
            byte == b'\\' && matches!(word.get(index + 1), Some(b'n' | b'r' | b't'));
        if escaped_space {
            index += 2;
            continue;
        }
        let plain = byte.is_ascii_alphabetic()
            || matches!(byte, b'_' | b'-')
            || byte.is_ascii_whitespace()
            || !byte.is_ascii();
        if !plain {
            return false;
        }
        index += 1;
    }
    true
}

fn is_mask(value: &[u8]) -> bool {
    value.windows(3).any(|window| window == b"***") || is_punctuation(value)
}

/// `os.environ/DB_PASSWORD`, `env:API_TOKEN`: the value ends in an upper-case name, set off
/// from what precedes it, that the key gate reads as naming a secret.
fn names_a_secret_variable(value: &[u8]) -> bool {
    let start = value
        .iter()
        .rposition(|byte| !(byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_'))
        .map_or(0, |index| index + 1);
    let name = &value[start..];
    let set_off = start == 0 || matches!(value[start - 1], b'/' | b':' | b'.');
    set_off && name.first().is_some_and(u8::is_ascii_uppercase) && has_secret_key_token(name)
}

/// A path relative to the home or working directory, or an absolute path whose last segment
/// carries a file extension. Base64 text can begin with `/`, so an absolute path needs the
/// extension.
fn is_file_path(value: &[u8]) -> bool {
    if value.starts_with(b"~/") || value.starts_with(b"./") || value.starts_with(b"../") {
        return true;
    }
    if value.first() != Some(&b'/') {
        return false;
    }
    let name = value.rsplit(|byte| *byte == b'/').next().unwrap_or(value);
    name.iter()
        .rposition(|byte| *byte == b'.')
        .is_some_and(|dot| {
            let extension = &name[dot + 1..];
            (1..=5).contains(&extension.len())
                && extension
                    .iter()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

// Both qualifier walks run once per token rather than once per candidate split, so cost stays linear in the token length instead of quadratic.
fn token_matches_any_key_name<Name: AsRef<[u8]>>(token: &[u8], names: &[Name]) -> bool {
    if names
        .iter()
        .any(|name| token.eq_ignore_ascii_case(name.as_ref()))
    {
        return true;
    }
    // A name has to occur inside the token before any qualifier chain can frame
    // it, and most tokens on a line hold none, so the two reach walks are skipped
    // for them.
    if !names
        .iter()
        .any(|name| find_ignore_ascii_case(token, name.as_ref()))
    {
        return false;
    }
    let from_start = qualifier_reach(token, false);
    let to_end = qualifier_reach(token, true);
    names.iter().any(|name| {
        let name = name.as_ref();
        let Some(last) = token.len().checked_sub(name.len()) else {
            return false;
        };
        (0..=last).any(|start| {
            from_start.get(start)
                && to_end.get(start + name.len())
                && token[start..start + name.len()].eq_ignore_ascii_case(name)
        })
    })
}

/// Positions of a token a qualifier chain reaches, as a bit set; positions
/// past the inline capacity spill to the heap.
struct Reach {
    inline: [u64; 2],
    spill: Vec<bool>,
}

impl Reach {
    const INLINE_BITS: usize = 128;

    fn new(len: usize) -> Self {
        Self {
            inline: [0; 2],
            spill: if len >= Self::INLINE_BITS {
                vec![false; len + 1 - Self::INLINE_BITS]
            } else {
                Vec::new()
            },
        }
    }

    fn set(&mut self, at: usize) {
        if at < Self::INLINE_BITS {
            self.inline[at / 64] |= 1u64 << (at % 64);
        } else {
            self.spill[at - Self::INLINE_BITS] = true;
        }
    }

    fn get(&self, at: usize) -> bool {
        if at < Self::INLINE_BITS {
            self.inline[at / 64] & (1u64 << (at % 64)) != 0
        } else {
            self.spill[at - Self::INLINE_BITS]
        }
    }
}

/// `Standard` match kind with overlapping iteration reports nested qualifiers
/// too (`app` inside `application`, `id` inside `ids`), which the reach walk
/// needs as separate spans.
static KEY_QUALIFIER_AUTOMATON: LazyLock<AhoCorasick> = LazyLock::new(|| {
    AhoCorasickBuilder::new()
        .match_kind(MatchKind::Standard)
        .ascii_case_insensitive(true)
        .kind(Some(AhoCorasickKind::DFA))
        .build(KEY_QUALIFIERS)
        .expect("key qualifier automaton")
});

/// Visiting spans in end order is a topological order for both directions:
/// forward, a span's start is reached from position 0 or through spans ending
/// there, which end earlier; backward, its end is reached from the token end
/// or through spans starting there, which end later.
fn qualifier_reach(token: &[u8], reverse: bool) -> Reach {
    let mut reach = Reach::new(token.len());
    reach.set(if reverse { token.len() } else { 0 });
    let mut spans: Vec<(usize, usize)> = KEY_QUALIFIER_AUTOMATON
        .find_overlapping_iter(token)
        .map(|hit| (hit.start(), hit.end()))
        .collect();
    spans.sort_unstable_by_key(|&(_, end)| end);
    if reverse {
        for &(start, end) in spans.iter().rev() {
            if reach.get(end) {
                reach.set(start);
            }
        }
    } else {
        for &(start, end) in &spans {
            if reach.get(start) {
                reach.set(end);
            }
        }
    }
    reach
}

fn key_tokens(key: &[u8]) -> impl Iterator<Item = &[u8]> {
    key.split(|byte| !byte.is_ascii_alphanumeric())
        .flat_map(|part| CamelTokens { remaining: part })
        .filter(|part| !part.is_empty())
}

struct CamelTokens<'a> {
    remaining: &'a [u8],
}

impl<'a> Iterator for CamelTokens<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }
        let split = (1..self.remaining.len()).find(|&index| {
            self.remaining[index].is_ascii_uppercase()
                && (self.remaining[index - 1].is_ascii_lowercase()
                    || self
                        .remaining
                        .get(index + 1)
                        .is_some_and(u8::is_ascii_lowercase))
        });
        let (token, remaining) = split.map_or((self.remaining, &[][..]), |index| {
            self.remaining.split_at(index)
        });
        self.remaining = remaining;
        Some(token)
    }
}

static DEFAULT_CHAR_CLASS: crate::rules::CharClassSpec = crate::rules::CharClassSpec {
    max_lower_pct: 95,
    min_window_len: 32,
};

fn lowercase_percent(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    bytes
        .iter()
        .filter(|byte| byte.is_ascii_lowercase())
        .count()
        .saturating_mul(100)
        / bytes.len()
}

enum EntropyOutcome {
    Failed,
    Bypassed,
    Passed,
}

fn entropy_allows(spec: &EntropySpec, value: &[u8]) -> EntropyOutcome {
    if value.len() < spec.min_len {
        return EntropyOutcome::Bypassed;
    }
    let value = &value[..value.len().min(spec.max_len)];
    let mut counts = [0usize; 256];
    for byte in value {
        counts[usize::from(*byte)] += 1;
    }
    let len = value.len() as f32;
    let mut shannon = 0.0f32;
    let mut max_count = 0usize;
    for count in counts.into_iter().filter(|count| *count > 0) {
        let probability = count as f32 / len;
        shannon -= probability * probability.log2();
        max_count = max_count.max(count);
    }
    if spec.digit_penalty && value.iter().all(u8::is_ascii_digit) && len > 1.0 {
        shannon -= 1.2 / len.log2();
    }
    if shannon < spec.min_bits_per_byte {
        return EntropyOutcome::Failed;
    }
    if spec
        .min_entropy_bits_per_byte
        .is_some_and(|minimum| len.log2() - (max_count as f32).log2() < minimum)
    {
        return EntropyOutcome::Failed;
    }
    EntropyOutcome::Passed
}

// `is_scalar` ignores ASCII case because languages capitalize null-like and boolean values differently; otherwise `password=None` is reported as a secret. The non-finite spellings carry their own sign because the decimal parser below never reaches them.
const SCALAR_KEYWORDS: &[&str] = &[
    "true",
    "false",
    "null",
    "undefined",
    "none",
    "nil",
    "nan",
    ".nan",
    "inf",
    "+inf",
    "-inf",
    ".inf",
    "+.inf",
    "-.inf",
    "infinity",
    "+infinity",
    "-infinity",
    "~",
];

// A number with a time unit is a duration, as in `access_token_ttl=900s`.
const DURATION_UNITS: &[&str] = &["ns", "us", "ms", "s", "m", "h", "d", "w"];

// Quoted overlay rules keep `\\.` sequences as content, so a value spelled `\n` arrives as two bytes that `trim` leaves in place.
fn is_blank_with_escapes(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        let escaped = bytes[index] == b'\\'
            && bytes
                .get(index + 1)
                .is_some_and(|byte| matches!(byte, b'n' | b't' | b'r' | b'f' | b'v' | b'0' | b' '));
        if !escaped {
            return false;
        }
        index += 2;
    }
    true
}

// Hex-encoded credentials are also `0x`-prefixed, so a longer digit run stays a
// candidate: an Ethereum private key carries 64 hex digits. Modes, masks, and
// flags fit inside this bound.
const MAX_RADIX_SCALAR_DIGITS: usize = 8;

// A separator only counts between digits, so `1_000` is a literal while `_1`,
// `1_`, and `1__0` are not.
fn consume_digit_run(
    bytes: &[u8],
    index: &mut usize,
    digits: &mut usize,
    accepts: fn(&u8) -> bool,
) {
    while let Some(byte) = bytes.get(*index) {
        if accepts(byte) {
            *index += 1;
            *digits += 1;
            continue;
        }
        let separated = *byte == b'_' && *digits > 0 && bytes.get(*index + 1).is_some_and(accepts);
        if !separated {
            return;
        }
        *index += 1;
    }
}

fn is_radix_scalar(bytes: &[u8]) -> bool {
    let bytes = match bytes.first() {
        Some(b'+' | b'-') => &bytes[1..],
        _ => bytes,
    };
    if bytes.first() != Some(&b'0') {
        return false;
    }
    let Some(radix) = bytes.get(1) else {
        return false;
    };
    let digits = &bytes[2..];
    let accepts: fn(&u8) -> bool = match radix.to_ascii_lowercase() {
        b'x' => u8::is_ascii_hexdigit,
        b'o' => |byte: &u8| (b'0'..=b'7').contains(byte),
        b'b' => |byte: &u8| matches!(byte, b'0' | b'1'),
        _ => return false,
    };
    let mut index = 0usize;
    let mut count = 0usize;
    consume_digit_run(digits, &mut index, &mut count, accepts);
    index == digits.len() && count > 0 && count <= MAX_RADIX_SCALAR_DIGITS
}

fn is_scalar(value: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(value) else {
        return false;
    };
    let text = text.trim();
    if SCALAR_KEYWORDS
        .iter()
        .any(|keyword| text.eq_ignore_ascii_case(keyword))
    {
        return true;
    }
    if is_blank_with_escapes(text) {
        return true;
    }
    let bytes = text.as_bytes();
    if is_radix_scalar(bytes) {
        return true;
    }
    let mut index = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    // Digits are counted across the point rather than required on its left, because `.5` and `5.` are decimal literals a configuration file can hold.
    let mut digits = 0usize;
    consume_digit_run(bytes, &mut index, &mut digits, u8::is_ascii_digit);
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        consume_digit_run(bytes, &mut index, &mut digits, u8::is_ascii_digit);
    }
    if digits == 0 {
        return false;
    }
    if DURATION_UNITS
        .iter()
        .any(|unit| bytes[index..].eq_ignore_ascii_case(unit.as_bytes()))
    {
        return true;
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let exponent_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }
    index == bytes.len()
}

// Longer segments are left to the secret rules: a random body such as a vendor token's dotted payload runs past this, while member names stay well under it.
const MAX_CODE_SEGMENT_BYTES: usize = 32;

// Digest widths such as `sha256` need three digits; a four-digit suffix is usually a year.
const MAX_CODE_SUFFIX_DIGITS: usize = 3;

fn names_code(gate: CodeReferenceGate, value: &[u8]) -> bool {
    if is_code_reference(value) {
        return true;
    }
    match gate {
        CodeReferenceGate::Assignment => false,
        // The upstream rule allowlists `^[a-zA-Z_.-]+$`, since a credential this generic carries a digit.
        CodeReferenceGate::GenericApiKey => {
            value
                .iter()
                .all(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'.' | b'-'))
                || value
                    .iter()
                    .position(|byte| *byte == b'=')
                    .is_some_and(|equals| {
                        let (name, rest) = (&value[..equals], &value[equals + 1..]);
                        // An empty right side is Base64 padding, as in `QxWvErTy4=`, and `is_scalar` admits blanks.
                        !rest.is_empty()
                            && is_identifier_path(name, 1)
                            && (is_scalar(rest) || is_code_reference(rest))
                    })
        }
    }
}

/// A qualified name such as `event.id` or `self.config.api_key`, a call or index
/// whose unquoted value stops at `(` or a quote (`request.headers.get`,
/// `os.environ[`), or a `{name}` template such as `{pipeline_name}_{YYYYMMDD}`.
/// One bare word is not a reference, because `password=hunter` is a literal.
fn is_code_reference(value: &[u8]) -> bool {
    let value = value.strip_suffix(b".").unwrap_or(value);
    let value = value.strip_suffix(b"[").unwrap_or(value);
    is_identifier_path(value, 2) || is_template(value)
}

fn is_identifier_path(value: &[u8], min_segments: usize) -> bool {
    let mut segments = 0usize;
    let mut rest = value;
    loop {
        let end = rest
            .iter()
            .position(|byte| matches!(byte, b'.' | b':'))
            .unwrap_or(rest.len());
        if !is_identifier(&rest[..end]) {
            return false;
        }
        segments += 1;
        rest = match &rest[end..] {
            [] => return segments >= min_segments,
            [b'.', tail @ ..] | [b':', b':', tail @ ..] => tail,
            _ => return false,
        };
    }
}

/// Digits may only close a segment, since a digit inside a word marks a random body.
/// A digit suffix must also be short and follow a lowercase word (`sha256`, `api_v2`),
/// because a capitalized or year-numbered word such as `River2024` is how people build passwords.
fn is_identifier(segment: &[u8]) -> bool {
    let Some(first) = segment.first() else {
        return false;
    };
    let word_end = segment
        .iter()
        .rposition(|byte| !byte.is_ascii_digit())
        .map_or(0, |index| index + 1);
    let (word, suffix) = segment.split_at(word_end);
    segment.len() <= MAX_CODE_SEGMENT_BYTES
        && (first.is_ascii_alphabetic() || *first == b'_')
        && word
            .iter()
            .all(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        && (suffix.is_empty()
            || (suffix.len() <= MAX_CODE_SUFFIX_DIGITS && !word.iter().any(u8::is_ascii_uppercase)))
}

fn is_template(value: &[u8]) -> bool {
    let mut placeholders = 0usize;
    let mut rest = value;
    while let Some(open) = rest.iter().position(|byte| *byte == b'{') {
        if !rest[..open].iter().all(is_template_literal_byte) {
            return false;
        }
        let Some(close) = rest[open..].iter().position(|byte| *byte == b'}') else {
            return false;
        };
        if !is_identifier(&rest[open + 1..open + close]) {
            return false;
        }
        placeholders += 1;
        rest = &rest[open + close + 1..];
    }
    placeholders > 0 && rest.iter().all(is_template_literal_byte)
}

fn is_template_literal_byte(byte: &u8) -> bool {
    matches!(byte, b'_' | b'-' | b'.' | b'/' | b':')
}

fn local_context_allows(
    spec: &LocalContextSpec,
    input: &[u8],
    value: TextSpan,
    work: &mut usize,
    limits: ScanLimits,
) -> Result<bool, Abort> {
    let start = value.start().saturating_sub(spec.lookbehind);
    let end = value.end().saturating_add(spec.lookahead).min(input.len());
    let Some(window) = input.get(start..end) else {
        return Ok(false);
    };
    let value_start = value.start() - start;
    let value_end = value.end() - start;
    if spec.require_quoted {
        let quoted = value_start
            .checked_sub(1)
            .and_then(|before| window.get(before).copied())
            .zip(window.get(value_end).copied())
            .is_some_and(|(left, right)| left == right && matches!(left, b'\'' | b'"' | b'`'));
        if !quoted {
            return Ok(false);
        }
    }
    let line_start = window[..value_start]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    let before = &window[line_start..value_start];
    if spec.require_same_line_assignment
        && !before.iter().any(|byte| matches!(byte, b'=' | b':' | b'>'))
    {
        return Ok(false);
    }
    if let Some(keys) = &spec.key_names_any {
        add_work(work, before.len(), limits.max_work_bytes)?;
        // Compared per identifier token rather than as a substring, so the `key` inside `monkey` and the `auth` inside `author` do not satisfy the gate.
        let matched = key_tokens(before).any(|token| token_matches_any_key_name(token, keys));
        return Ok(matched);
    }
    Ok(true)
}

fn is_uuid(value: &[u8]) -> bool {
    value.len() == 36
        && value.iter().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23) && *byte == b'-'
                || !matches!(index, 8 | 13 | 18 | 23) && byte.is_ascii_hexdigit()
        })
}

#[derive(Clone, Copy)]
enum OfflineVerdict {
    Valid,
    Invalid,
    Indeterminate,
}

fn offline_verdict(spec: &OfflineValidationSpec, value: &[u8]) -> OfflineVerdict {
    match spec.kind {
        OfflineValidationKind::Crc32Base62 => {
            let (Some(skip), Some(payload_len), Some(checksum_len)) =
                (spec.prefix_skip, spec.payload_len, spec.checksum_len)
            else {
                return OfflineVerdict::Indeterminate;
            };
            validate_crc32_base62(
                value,
                usize::from(skip),
                usize::from(payload_len),
                usize::from(checksum_len),
            )
        }
        OfflineValidationKind::GithubFineGrainedPat => {
            if value.len() < 93 || !starts_with_ignore_ascii_case(value, b"github_pat_") {
                OfflineVerdict::Indeterminate
            } else {
                crc_verdict(&value[..87], &value[87..93])
            }
        }
        // Compared case-insensitively because the rule pattern uses `(?i)`. A case-sensitive test returns `Indeterminate` for `GLSA_…`, which does not reject.
        OfflineValidationKind::GrafanaServiceAccount => {
            if value.len() < 46
                || !starts_with_ignore_ascii_case(value, b"glsa_")
                || value.get(37) != Some(&b'_')
            {
                OfflineVerdict::Indeterminate
            } else {
                checksum_verdict(&value[..37], parse_hex_u32(&value[38..46]))
            }
        }
        OfflineValidationKind::AwsAccessKey => validate_aws(value),
        OfflineValidationKind::SentryOrgToken => validate_sentry(value),
        OfflineValidationKind::PypiToken => validate_pypi(value),
        OfflineValidationKind::SlackToken => validate_slack(value),
        OfflineValidationKind::BasicCredentials => validate_basic_credentials(value),
    }
}

/// RFC 7617 credentials are the Base64 of `user-id ":" password` in the charset the server
/// expects, so the value has to decode to bytes that hold a `:` and are free of ASCII control
/// characters. A word after `Basic` in prose (`Authorization: Basic authentication`) decodes
/// to bytes that fail one of the two.
fn validate_basic_credentials(value: &[u8]) -> OfflineVerdict {
    // `echo user:password | base64` encodes the line break `echo` adds.
    let credentials = decode_base64(value).filter(|decoded| {
        let text = decoded
            .strip_suffix(b"\r\n")
            .or_else(|| decoded.strip_suffix(b"\n"))
            .unwrap_or(decoded);
        text.contains(&b':') && !text.iter().any(u8::is_ascii_control)
    });
    match credentials {
        Some(_) => OfflineVerdict::Valid,
        None => OfflineVerdict::Invalid,
    }
}

/// Decodes the standard or the URL-safe alphabet, padded or bare.
/// `None` when a byte is outside both alphabets or the length leaves a lone sextet.
fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    let body_len = input
        .iter()
        .rposition(|byte| *byte != b'=')
        .map_or(0, |index| index + 1);
    let body = &input[..body_len];
    if body.len() % 4 == 1 {
        return None;
    }
    let mut output = Vec::with_capacity(body.len() / 4 * 3 + 2);
    let mut bits = 0u32;
    let mut bit_count = 0u8;
    for byte in body {
        let value = match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'a'..=b'z' => u32::from(byte - b'a' + 26),
            b'0'..=b'9' => u32::from(byte - b'0' + 52),
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        bits = (bits << 6) | value;
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            output.push((bits >> bit_count) as u8);
            bits &= (1u32 << bit_count) - 1;
        }
    }
    Some(output)
}

fn validate_crc32_base62(
    value: &[u8],
    skip: usize,
    payload: usize,
    checksum: usize,
) -> OfflineVerdict {
    let Some(required) = skip
        .checked_add(payload)
        .and_then(|size| size.checked_add(checksum))
    else {
        return OfflineVerdict::Indeterminate;
    };
    let Some(body) = value.get(skip..skip + payload) else {
        return OfflineVerdict::Indeterminate;
    };
    let Some(encoded) = value.get(skip + payload..required) else {
        return OfflineVerdict::Indeterminate;
    };
    crc_verdict(body, encoded)
}

fn crc_verdict(body: &[u8], encoded: &[u8]) -> OfflineVerdict {
    checksum_verdict(body, base62_u32(encoded))
}

// A checksum that does not decode as `u32` cannot represent a CRC32, so malformed or overflowing base-62 fields are invalid rather than unverifiable.
fn checksum_verdict(body: &[u8], expected: Option<u32>) -> OfflineVerdict {
    match expected {
        Some(expected) if crc32(body) == expected => OfflineVerdict::Valid,
        Some(_) | None => OfflineVerdict::Invalid,
    }
}

fn starts_with_ignore_ascii_case(value: &[u8], prefix: &[u8]) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn base62_u32(bytes: &[u8]) -> Option<u32> {
    let mut value = 0u64;
    for byte in bytes {
        let digit = match byte {
            b'0'..=b'9' => u64::from(byte - b'0'),
            b'A'..=b'Z' => u64::from(byte - b'A' + 10),
            b'a'..=b'z' => u64::from(byte - b'a' + 36),
            _ => return None,
        };
        value = value.checked_mul(62)?.checked_add(digit)?;
    }
    u32::try_from(value).ok()
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn parse_hex_u32(bytes: &[u8]) -> Option<u32> {
    if bytes.len() != 8 {
        return None;
    }
    let mut value = 0u32;
    for byte in bytes {
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' => u32::from(byte - b'a' + 10),
            b'A'..=b'F' => u32::from(byte - b'A' + 10),
            _ => return None,
        };
        value = value.checked_mul(16)?.checked_add(digit)?;
    }
    Some(value)
}

fn validate_aws(value: &[u8]) -> OfflineVerdict {
    if value.len() < 20 {
        return OfflineVerdict::Indeterminate;
    }
    let key = &value[..20];
    if !(key.starts_with(b"AKIA")
        || key.starts_with(b"ASIA")
        || key.starts_with(b"ABIA")
        || key.starts_with(b"ACCA")
        || key.starts_with(b"A3T"))
    {
        return OfflineVerdict::Indeterminate;
    }
    if key[4..]
        .iter()
        .all(|byte| byte.is_ascii_uppercase() || (b'2'..=b'7').contains(byte))
    {
        OfflineVerdict::Valid
    } else {
        OfflineVerdict::Invalid
    }
}

fn validate_sentry(value: &[u8]) -> OfflineVerdict {
    if !value.starts_with(b"sntrys_") {
        return OfflineVerdict::Indeterminate;
    }
    let body = &value[7..];
    let Some(separator) = body.iter().rposition(|byte| *byte == b'_') else {
        return OfflineVerdict::Indeterminate;
    };
    if body.len().saturating_sub(separator + 1) < 43 {
        return OfflineVerdict::Indeterminate;
    }
    if base64_prefix(&body[..separator], b"{\"iat\":") {
        OfflineVerdict::Valid
    } else {
        OfflineVerdict::Invalid
    }
}

fn validate_pypi(value: &[u8]) -> OfflineVerdict {
    const HEADER: &[u8] = b"\x02\x01\x08pypi.org\x02";
    if !value.starts_with(b"pypi-") || value.len() < 21 {
        return OfflineVerdict::Indeterminate;
    }
    if base64url_prefix(&value[5..], HEADER) {
        OfflineVerdict::Valid
    } else {
        OfflineVerdict::Invalid
    }
}

// Unsupported prefixes and unparsable `xoxe` values return Indeterminate; malformed shapes for supported prefixes return Invalid. `xapp` requires only its first segment to start with a digit, while `xoxa` and `xoxr` allow one alphanumeric segment.
// The other prefixes check only the all-digit segments every Slack rule regex fixes before the opaque tail: one for `xoxb` and the `xoxe.xox` config token, three for `xoxp`, `xoxo`, and `xoxs`, and for `xoxe` one in its two-segment refresh shape and three otherwise. The tail may hold `-`, so the verdict never depends on where a hyphen falls in it.
fn validate_slack(value: &[u8]) -> OfflineVerdict {
    if value.len() < 10 {
        return OfflineVerdict::Indeterminate;
    }
    let prefix = &value[..4];
    let config_access = starts_with_ignore_ascii_case(value, b"xoxe.xox");
    let body_start = if value.get(4) == Some(&b'-') {
        5
    } else if config_access {
        match value.iter().position(|byte| *byte == b'-') {
            Some(dash) => dash + 1,
            None => return OfflineVerdict::Indeterminate,
        }
    } else {
        return OfflineVerdict::Indeterminate;
    };
    let body = &value[body_start..];
    let mut segments = body.split(|byte| *byte == b'-');
    let shaped = if matches!(prefix, b"xoxa" | b"xoxr") {
        segments.next().is_some_and(|segment| {
            !segment.is_empty() && segment.iter().all(u8::is_ascii_alphanumeric)
        })
    } else if prefix.eq_ignore_ascii_case(b"xapp") {
        segments
            .next()
            .is_some_and(|segment| segment.first().is_some_and(u8::is_ascii_digit))
    } else if config_access || prefix == b"xoxb" {
        after_digit_segments(body, 1).is_some()
    } else if matches!(prefix, b"xoxp" | b"xoxo" | b"xoxs") {
        after_digit_segments(body, 3).is_some()
    } else if prefix.eq_ignore_ascii_case(b"xoxe") {
        // The two-segment refresh shape holds exactly one `-` in its body.
        after_digit_segments(body, 1).is_some_and(|rest| {
            after_digit_segments(rest, 2).is_some() || memchr::memchr(b'-', rest).is_none()
        })
    } else {
        return OfflineVerdict::Indeterminate;
    };
    if shaped {
        OfflineVerdict::Valid
    } else {
        OfflineVerdict::Invalid
    }
}

fn after_digit_segments(mut body: &[u8], count: usize) -> Option<&[u8]> {
    for _ in 0..count {
        let digits = body.iter().take_while(|byte| byte.is_ascii_digit()).count();
        if digits == 0 || body.get(digits) != Some(&b'-') {
            return None;
        }
        body = &body[digits + 1..];
    }
    Some(body)
}

fn base64_prefix(input: &[u8], prefix: &[u8]) -> bool {
    decode_prefix(input, prefix, false)
}

fn base64url_prefix(input: &[u8], prefix: &[u8]) -> bool {
    decode_prefix(input, prefix, true)
}

fn decode_prefix(input: &[u8], prefix: &[u8], url: bool) -> bool {
    let mut bits = 0u32;
    let mut bit_count = 0u8;
    let mut output = 0usize;
    for byte in input {
        let value = match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'a'..=b'z' => u32::from(byte - b'a' + 26),
            b'0'..=b'9' => u32::from(byte - b'0' + 52),
            b'+' if !url => 62,
            b'/' if !url => 63,
            b'-' if url => 62,
            b'_' if url => 63,
            b'=' if !url => continue,
            _ => return false,
        };
        bits = (bits << 6) | value;
        bit_count += 6;
        while bit_count >= 8 {
            bit_count -= 8;
            let decoded = (bits >> bit_count) as u8;
            if prefix.get(output) != Some(&decoded) {
                return false;
            }
            output += 1;
            if output == prefix.len() {
                return true;
            }
            bits &= (1u32 << bit_count) - 1;
        }
    }
    false
}

#[cfg(test)]
mod kernels;
#[cfg(test)]
mod monotonicity;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_classifier_is_closed() {
        for scalar in [
            "0",
            "-1.5",
            "+2e10",
            ".5",
            "-.25",
            "+.5",
            ".5e3",
            "5.",
            "-5.",
            "true",
            "false",
            "null",
            "undefined",
            "True",
            "FALSE",
            "None",
            "nil",
            "NaN",
            "~",
            "",
            "   ",
            "\t\n",
            "\\n",
            "\\t",
            "\\r\\n",
            " \\n ",
            "0x10",
            "0X1F",
            "0o755",
            "0b1010",
            "-0x10",
            "0xdeadbeef",
            "900s",
            "86400s",
            "250ms",
            "1.5h",
            "90d",
        ] {
            assert!(is_scalar(scalar.as_bytes()), "{scalar:?}");
        }
        for secret in [
            "12x",
            "900x",
            "9s9",
            "s",
            "hunter2",
            "true-blue",
            "nilpotent",
            "nonement",
            ".",
            "-.",
            ".x",
            "5.5.5",
            "0x",
            "0xz",
            "0o8",
            "0b2",
            "0x123456789",
            "0xdeadbeefcafe1234",
            "\\\\",
            "\\nhunter2",
            "\\x",
        ] {
            assert!(!is_scalar(secret.as_bytes()), "{secret:?}");
        }
    }

    #[test]
    fn code_reference_gate_is_closed() {
        for gate in [
            CodeReferenceGate::Assignment,
            CodeReferenceGate::GenericApiKey,
        ] {
            for code in [
                "event.id",
                "self.config.api_key",
                "settings::DB_PASSWORD",
                "refreshedClaims.TenantRoles",
                "os.environ[",
                "partition_epoch.id.",
                "self.hashes.sha256",
                "client.oauth2.token",
                "settings.api_v2",
                "{pipeline_name}_{YYYYMMDD}",
            ] {
                assert!(names_code(gate, code.as_bytes()), "{gate:?} {code:?}");
            }
            for secret in [
                "hunter2",
                "aB3xZ9qL.Kp4mN2vT",
                "Cobalt.River2024",
                "Cobalt.River123",
                "cobalt.river2024",
                "QxWvErTyUiOpAsDfGhJkLz4=",
                "{user}_Xk39fJ2qLp0Z",
            ] {
                assert!(!names_code(gate, secret.as_bytes()), "{gate:?} {secret:?}");
            }
        }
        for code in [
            "access_token_ttl=900",
            "active_membership_id.",
            "name=event.id",
        ] {
            assert!(
                names_code(CodeReferenceGate::GenericApiKey, code.as_bytes()),
                "{code:?}"
            );
        }
    }

    #[test]
    fn crc32_matches_standard_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    /// The semantic digest does not cover these tables, so a change here must also bump `REVISION.semantic_digest_version`; update the pinned digest in the same change.
    #[test]
    fn evaluator_constants_are_pinned() {
        let mut encoded = Vec::new();
        fn push_table<T: AsRef<[u8]>>(encoded: &mut Vec<u8>, table: &[T]) {
            for entry in table {
                encoded.extend_from_slice(entry.as_ref());
                encoded.push(0);
            }
            encoded.push(0xff);
        }
        push_table(&mut encoded, SECRET_KEY_WORDS);
        push_table(&mut encoded, KEY_QUALIFIERS);
        push_table(&mut encoded, NON_SECRET_KEY_MARKERS);
        push_table(&mut encoded, SCALAR_KEYWORDS);
        push_table(&mut encoded, DURATION_UNITS);
        push_table(&mut encoded, &KEYED_KEYWORDS);
        push_table(&mut encoded, MEMORIZED_SECRET_WORDS);
        push_table(&mut encoded, &[PASSWORD_SUFFIX]);
        encoded.extend_from_slice(&(MIN_PIN_DIGITS as u64).to_le_bytes());
        encoded.extend_from_slice(&(MAX_CONTINUATION_BYTES as u64).to_le_bytes());
        encoded.extend_from_slice(&(MAX_ESCAPE_RUN_BYTES as u64).to_le_bytes());
        push_table(&mut encoded, NON_NULL_SCALAR_TYPES);
        encoded.extend_from_slice(&(MAX_MATCH_BYTES as u64).to_le_bytes());
        encoded.extend_from_slice(&(MAX_RADIX_SCALAR_DIGITS as u64).to_le_bytes());
        encoded.extend_from_slice(&(MAX_CODE_SEGMENT_BYTES as u64).to_le_bytes());
        encoded.extend_from_slice(&(MAX_CODE_SUFFIX_DIGITS as u64).to_le_bytes());
        encoded.push(DEFAULT_CHAR_CLASS.max_lower_pct);
        encoded.extend_from_slice(&DEFAULT_CHAR_CLASS.min_window_len.to_le_bytes());
        assert_eq!(
            crate::rules::digest_hex(&encoded),
            "1f21913e0613ef2909accfa2c83aca9b713e0f0128179ff95882bc8b5d2a1712",
            "evaluator constants changed: bump REVISION.semantic_digest_version and re-pin"
        );
    }

    fn alternation_rule(declaration: &str) -> Rule {
        let declaration: crate::rules::RuleDeclaration = serde_json::from_str(declaration).unwrap();
        crate::rules::compile_rule(RuleSource::ConservativeOverlay, declaration).unwrap()
    }

    const KEYED_RULE_NAMES: [&str; 7] = [
        "magic-keyed-assignment",
        "magic-keyed-assignment-double-quoted",
        "magic-keyed-assignment-single-quoted",
        "magic-keyed-double-quoted",
        "magic-keyed-single-quoted",
        "magic-keyed-double-single",
        "magic-keyed-single-double",
    ];

    const RULE_SPACE_BYTES: [u8; 6] = [b'\t', b'\n', 0x0B, b'\x0C', b'\r', b' '];

    const INLINE_SPACE_BYTES: [u8; 4] = [b'\t', 0x0B, b'\x0C', b' '];

    const UNQUOTED_VALUE_TERMINATORS: [u8; 11] = *b"'\"`;&|<>()$";

    fn embedded_rule<'a>(rules: &'a RuleSet, name: &str) -> &'a Rule {
        rules
            .active(ScanProfile::Comprehensive)
            .find(|rule| rule.declaration.name == name)
            .unwrap_or_else(|| panic!("no embedded rule named {name}"))
    }

    /// Every regex match, including the empty-value ones `candidate_spans` counts but does not evaluate.
    fn regex_candidates(rule: &Rule, input: &str) -> Vec<CandidateSpans> {
        rule.regex
            .captures_iter(input.as_bytes())
            .map(|captures| {
                let group = |name: &str| {
                    let found = captures.name(name).unwrap();
                    TextSpan::snapped(input, found.start(), found.end()).unwrap()
                };
                let full = captures.get(0).unwrap();
                let value = group("value");
                let spans = CandidateSpans {
                    full: TextSpan::snapped(input, full.start(), full.end()).unwrap(),
                    value,
                    reported: value,
                    key: Some(group("key")),
                };
                let evaluated = candidate_spans(rule, &captures, input).unwrap();
                assert_eq!(evaluated, (!spans.value.is_empty()).then_some(spans));
                spans
            })
            .collect()
    }

    /// Replays the probe loop in `evaluate` and reports the candidates with the number of probes it took.
    fn fast_path_candidates(form: KeyedForm, input: &str) -> (Vec<CandidateSpans>, usize) {
        let bytes = input.as_bytes();
        let mut candidates = Vec::new();
        let mut probes = 0;
        let mut cursor = 0;
        while let Some(at) =
            memchr::memchr(form.anchor(), &bytes[cursor..]).map(|offset| cursor + offset)
        {
            probes += 1;
            match form.probe(input, at, cursor).unwrap() {
                Probe::Candidate(spans) => {
                    cursor = spans.full.end();
                    candidates.push(spans);
                }
                Probe::Skip(next) => {
                    assert!(next > at, "{form:?} did not advance past {at} in {input:?}");
                    cursor = next;
                }
            }
        }
        (candidates, probes)
    }

    fn assert_fast_path_replays_embedded_regex(rules: &RuleSet, input: &str) {
        for name in KEYED_RULE_NAMES {
            let rule = embedded_rule(rules, name);
            let form = KeyedForm::for_rule(name).unwrap();
            let (actual, _) = fast_path_candidates(form, input);
            assert_eq!(actual, regex_candidates(rule, input), "{name} on {input:?}");
        }
    }

    /// Every `magic-keyed-*` rule in the overlay has a fast path, and every fast path names an overlay rule.
    #[test]
    fn keyed_forms_and_embedded_keyed_rules_correspond() {
        let rules = RuleSet::from_embedded().unwrap();
        let mut embedded: Vec<_> = rules
            .active(ScanProfile::Comprehensive)
            .map(|rule| rule.declaration.name.as_str())
            .filter(|name| name.starts_with("magic-keyed"))
            .collect();
        embedded.sort_unstable();
        let mut forms = KEYED_RULE_NAMES.to_vec();
        forms.sort_unstable();
        assert_eq!(embedded, forms);
        for name in KEYED_RULE_NAMES {
            assert!(
                KeyedForm::for_rule(name).is_some(),
                "{name} has no fast path"
            );
        }
    }

    // `KeyedForm::probe` hand-codes each regex below, so a change to one of them must be paired with a change to the fast path.
    #[test]
    fn keyed_rule_regexes_are_pinned_to_the_fast_path() {
        let rules = RuleSet::from_embedded().unwrap();
        for (name, regex) in [
            (
                "magic-keyed-assignment",
                r#"(?i)(?-u:\b)(?P<key>[A-Za-z0-9_.-]*(?:key|token|secret|password|auth|bearer|credential)[A-Za-z0-9_.-]*)(?:=|[\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]+=[\t\x0B\f \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]*)(?P<value>(?:[^\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}'"`;&|<>()$\\=]|\\.)(?:[^\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}'"`;&|<>()$\\]|\\.)*)"#,
            ),
            (
                "magic-keyed-assignment-double-quoted",
                r#"(?i)(?-u:\b)(?P<key>[A-Za-z0-9_.-]*(?:key|token|secret|password|auth|bearer|credential)[A-Za-z0-9_.-]*)[\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]*=[\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]*"(?P<value>(?:[^"\\]|\\.)*)""#,
            ),
            (
                "magic-keyed-assignment-single-quoted",
                r#"(?i)(?-u:\b)(?P<key>[A-Za-z0-9_.-]*(?:key|token|secret|password|auth|bearer|credential)[A-Za-z0-9_.-]*)[\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]*=[\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]*'(?P<value>(?:[^'\\]|\\.)*)'"#,
            ),
            (
                "magic-keyed-double-quoted",
                r#"(?i)"(?P<key>(?:[^"\\]|\\.)*(?:key|token|secret|password|auth|bearer|credential)(?:[^"\\]|\\.)*)"[\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]*:[\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]*"(?P<value>(?:[^"\\]|\\.)*)""#,
            ),
            (
                "magic-keyed-single-quoted",
                r#"(?i)'(?P<key>(?:[^'\\]|\\.)*(?:key|token|secret|password|auth|bearer|credential)(?:[^'\\]|\\.)*)'[\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]*:[\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]*'(?P<value>(?:[^'\\]|\\.)*)'"#,
            ),
            (
                "magic-keyed-double-single",
                r#"(?i)"(?P<key>(?:[^"\\]|\\.)*(?:key|token|secret|password|auth|bearer|credential)(?:[^"\\]|\\.)*)"[\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]*:[\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]*'(?P<value>(?:[^'\\]|\\.)*)'"#,
            ),
            (
                "magic-keyed-single-double",
                r#"(?i)'(?P<key>(?:[^'\\]|\\.)*(?:key|token|secret|password|auth|bearer|credential)(?:[^'\\]|\\.)*)'[\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]*:[\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]*"(?P<value>(?:[^"\\]|\\.)*)""#,
            ),
        ] {
            assert_eq!(
                embedded_rule(&rules, name).declaration.regex,
                regex,
                "{name} changed; update `KeyedForm::probe` and its differential tests"
            );
        }
    }

    #[test]
    fn keyed_fast_paths_replay_embedded_regex_spans() {
        let rules = RuleSet::from_embedded().unwrap();
        for input in [
            "password=hunter2",
            "API_TOKEN = Ab3fGh1jKlMnOpQrStUv",
            "prefix password=abc\\ def next",
            "password=first token=second",
            "monkey=value",
            "xpassword=value",
            ".token=value",
            "-token=value",
            "a.password=value",
            "a-.password=value",
            "1.2.token=value",
            "token=one;password=two",
            "token=\"quoted\"",
            "token=single'quoted",
            "token=",
            "key=\"\"",
            "token=''",
            "\"key\":\"\"",
            "token=one\\",
            "token=\\",
            "token=one\\ two key=three",
            "token=abcdefghij12345\\\nplaceholder",
            "token=abcdefghij12345\\\nmore",
            "token=abcdefghij12345\\\rmore",
            "password = token = abcdefghij12345",
            "k=key = key = v",
            "password\x0B=secret",
            "password=\x0Bsecret",
            "api_key = \"secret\\\"value\" token=\"next\"",
            "api_key = 'secret\\'value' token='next'",
            "api_key = \"abcdefghij12345\\\nmore\"",
            "api_key = 'abcdefghij12345\\\nmore'",
            "api_key = \"abc\\",
            "api_key = \"abc",
            "password=\"\\\"password=\\\"password=\\\"",
            "\"api\\\"key\": \"secret\\\"value\" \"token\":\"next\"",
            "'api\\'key': 'secret\\'value' 'token':'next'",
            "\"api\\\"key\": 'secret\\'value' \"token\":'next'",
            "'api\\'key': \"secret\\\"value\" 'token':\"next\"",
            "\"a\\key\": \"Ab3fGh1jKlMnOpQrStUv\"",
            "\"\\\\key\": \"Ab3fGh1jKlMnOpQrStUv\"",
            "\"key\": \"AAAA\\\nBBBB\"password\": \"AbCdEf1234567890x\"",
            "\"a\\\n\"key\":\"v\"",
            "\"x\" \"password\":\"v\"",
            "\"x\\\" \"key\":\"v\"",
            "\"key\": x \"key\": x \"key\": \"v\"",
            "\"key\"\x0B:\"v\"",
            "\"key\": \"\\\"\\\"\\\"",
            "\"\\\"\\\"\\\"key\": x",
            "'password':\"v\" \"password\":'v'",
        ] {
            assert_fast_path_replays_embedded_regex(&rules, input);
        }
    }

    // Each rule's separators, keywords, and terminators are hand-coded in the fast path; the embedded regex is the oracle.
    #[test]
    fn keyed_fast_paths_accept_every_keyword_and_separator_the_rules_accept() {
        let rules = RuleSet::from_embedded().unwrap();
        let secret = "hunter2AbCdEf123456";
        for keyword in KEYED_KEYWORDS {
            for separator in RULE_SPACE_BYTES {
                let gap = char::from(separator);
                let inline = INLINE_SPACE_BYTES.contains(&separator);
                // A line feed or a carriage return after `=` detaches an unquoted value, which leaves it to `magic-detached-assignment`.
                let detached = format!("{keyword}{gap}={gap}x{secret}");
                if !inline {
                    let rule = embedded_rule(&rules, "magic-keyed-assignment");
                    assert_eq!(regex_candidates(rule, &detached), [], "{detached:?}");
                    assert_fast_path_replays_embedded_regex(&rules, &detached);
                }
                for (form, input) in [
                    (
                        "magic-keyed-assignment",
                        format!("x_{keyword}{gap}=x{secret}"),
                    ),
                    (
                        "magic-keyed-assignment-double-quoted",
                        format!("{keyword}={gap}\"x{secret}\""),
                    ),
                    (
                        "magic-keyed-assignment-single-quoted",
                        format!("{keyword}{gap}='x{secret}'"),
                    ),
                ]
                .into_iter()
                .chain(inline.then(|| ("magic-keyed-assignment", detached.clone())))
                .chain([
                    (
                        "magic-keyed-double-quoted",
                        format!("\"{keyword}\"{gap}:\"x{secret}\""),
                    ),
                    (
                        "magic-keyed-single-quoted",
                        format!("'{keyword}'{gap}:'x{secret}'"),
                    ),
                    (
                        "magic-keyed-double-single",
                        format!("\"{keyword}\":{gap}'x{secret}'"),
                    ),
                    (
                        "magic-keyed-single-double",
                        format!("'{keyword}':{gap}\"x{secret}\""),
                    ),
                ]) {
                    let rule = embedded_rule(&rules, form);
                    let expected = regex_candidates(rule, &input);
                    assert_eq!(expected.len(), 1, "{form} does not match {input:?}");
                    assert_fast_path_replays_embedded_regex(&rules, &input);
                    let report = evaluate(
                        &rules,
                        ScanProfile::Conservative,
                        ScanLimits::default(),
                        [0u8; 32],
                        &input,
                    )
                    .unwrap();
                    let reported: Vec<_> = report
                        .findings
                        .iter()
                        .filter(|finding| finding.rule_id == form)
                        .map(|finding| finding.value_span)
                        .collect();
                    assert_eq!(reported, [expected[0].value], "{form} on {input:?}");
                }
            }
        }
    }

    #[test]
    fn keyed_fast_path_stops_unquoted_values_at_every_terminator_the_rule_stops_at() {
        let rules = RuleSet::from_embedded().unwrap();
        let rule = embedded_rule(&rules, "magic-keyed-assignment");
        for terminator in UNQUOTED_VALUE_TERMINATORS
            .into_iter()
            .chain(RULE_SPACE_BYTES)
        {
            // A space keeps the tail out of the `;` continuation, which belongs to this rule's findings and is absent from its candidates.
            let input = format!("password=hunter2AbCdEf{} tail", char::from(terminator));
            let expected = regex_candidates(rule, &input);
            assert_eq!(expected.len(), 1, "{input:?}");
            assert_eq!(
                &input[expected[0].value.start()..expected[0].value.end()],
                "hunter2AbCdEf",
                "{input:?}"
            );
            assert_fast_path_replays_embedded_regex(&rules, &input);
        }
        for content in *b"#,./:=@\\" {
            let input = format!("password=hunter2{}AbCdEf", char::from(content));
            let expected = regex_candidates(rule, &input);
            assert_eq!(expected.len(), 1, "{input:?}");
            assert_fast_path_replays_embedded_regex(&rules, &input);
        }
    }

    // A probe that restarts one byte after a rejected anchor rescans every escaped quote in the rejected region, which is quadratic in the input.
    #[test]
    fn keyed_fast_paths_probe_each_rejected_region_once() {
        let rules = RuleSet::from_embedded().unwrap();
        let escaped_quotes = "\\\"".repeat(2_000);
        for (name, input, max_probes) in [
            (
                "magic-keyed-double-quoted",
                format!("\"key\"{escaped_quotes}"),
                3,
            ),
            (
                "magic-keyed-double-quoted",
                format!("\"key\": \"{escaped_quotes}"),
                4,
            ),
            (
                "magic-keyed-double-quoted",
                format!("\"{escaped_quotes}key\": x"),
                3,
            ),
            (
                "magic-keyed-assignment-double-quoted",
                format!("password=\"{}", "\\\"password=".repeat(2_000)),
                1,
            ),
            (
                "magic-keyed-double-quoted",
                format!("\"a\\\n{}", "\"key\": x".repeat(2_000)),
                4_001,
            ),
        ] {
            let form = KeyedForm::for_rule(name).unwrap();
            let (actual, probes) = fast_path_candidates(form, &input);
            assert!(
                probes <= max_probes,
                "{name} took {probes} probes on {input:.40?}"
            );
            assert_eq!(
                actual,
                regex_candidates(embedded_rule(&rules, name), &input)
            );
        }
    }

    // Pieces that exercise every keyword, separator, quote, escape, and terminator the keyed rules distinguish.
    const KEYED_INPUT_PIECES: [&str; 19] = [
        "key", "TOKEN", "Password", "=", ":", "\"", "'", "\\", "\n", "\x0B", " ", "\t", ";", "$",
        ".", "-", "_", "a", "Z9",
    ];

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(2_000))]
        #[test]
        fn keyed_fast_paths_replay_embedded_regex_spans_on_generated_input(
            pieces in proptest::collection::vec(0..KEYED_INPUT_PIECES.len(), 0..24)
        ) {
            let input: String = pieces.iter().map(|&piece| KEYED_INPUT_PIECES[piece]).collect();
            assert_fast_path_replays_embedded_regex(&EMBEDDED_RULES_FOR_TESTS, &input);
        }
    }

    static EMBEDDED_RULES_FOR_TESTS: std::sync::LazyLock<RuleSet> =
        std::sync::LazyLock::new(|| RuleSet::from_embedded().unwrap());

    /// `qualifier_reach` must agree with this reference walk on every position of every token.
    fn qualifier_reach_oracle(token: &[u8], reverse: bool) -> Vec<bool> {
        let mut reach = vec![false; token.len() + 1];
        let origin = if reverse { token.len() } else { 0 };
        reach[origin] = true;
        for step in 0..token.len() {
            let at = if reverse { token.len() - step } else { step };
            if !reach[at] {
                continue;
            }
            for qualifier in KEY_QUALIFIERS {
                let span = if reverse {
                    at.checked_sub(qualifier.len()).map(|start| (start, at))
                } else {
                    (at + qualifier.len() <= token.len()).then_some((at, at + qualifier.len()))
                };
                let Some((start, end)) = span else {
                    continue;
                };
                if token[start..end].eq_ignore_ascii_case(qualifier) {
                    reach[if reverse { start } else { end }] = true;
                }
            }
        }
        reach
    }

    fn assert_qualifier_reach_matches_oracle(token: &[u8]) {
        for reverse in [false, true] {
            let reach = qualifier_reach(token, reverse);
            let oracle = qualifier_reach_oracle(token, reverse);
            let observed: Vec<bool> = (0..=token.len()).map(|at| reach.get(at)).collect();
            assert_eq!(observed, oracle, "reverse={reverse} token={token:?}");
        }
    }

    // Overlapping qualifiers, so one position can be reached through several chains.
    const QUALIFIER_INPUT_PIECES: [&str; 16] = [
        "api", "APP", "lication", "auth", "o", "id", "s", "hash", "es", "key", "x", "Db", "name",
        "value", "1", "_",
    ];

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(2_000))]
        #[test]
        fn qualifier_reach_matches_the_reference_walk(
            pieces in proptest::collection::vec(0..QUALIFIER_INPUT_PIECES.len(), 0..40)
        ) {
            let token: String = pieces.iter().map(|&piece| QUALIFIER_INPUT_PIECES[piece]).collect();
            assert_qualifier_reach_matches_oracle(token.as_bytes());
        }
    }

    #[test]
    fn qualifier_reach_matches_the_reference_walk_past_the_inline_capacity() {
        for token in [
            "api".repeat(Reach::INLINE_BITS),
            format!("{}key{}", "auth".repeat(40), "id".repeat(70)),
            "x".repeat(Reach::INLINE_BITS + 1),
            String::new(),
            "key".to_owned(),
        ] {
            assert_qualifier_reach_matches_oracle(token.as_bytes());
        }
    }

    // `evaluate` skips `hashicorp-tf-password` when the input holds no double quote; the rule's value group requires one.
    #[test]
    fn hashicorp_password_skip_requires_a_double_quote_in_every_match() {
        let rules = RuleSet::from_embedded().unwrap();
        let rule = embedded_rule(&rules, "hashicorp-tf-password");
        assert_eq!(
            rule.declaration.regex,
            r#"(?i)[\w.-]{0,50}?(?:administrator_login_password|password)(?:[ \t\w.-]{0,20})[\s'"]{0,3}(?:=|>|:{1,3}=|\|\||:|=>|\?=|,)[\x60'"\s=]{0,5}("[a-z0-9=_\-]{8,20}")(?:[\x60'"\s;]|\\[nr]|$)"#,
            "hashicorp-tf-password changed; recheck the double-quote skip in `evaluate`"
        );
        let quoted = r#"password = "kq7v2m9xw4zt""#;
        assert_eq!(rule.regex.captures_iter(quoted.as_bytes()).count(), 1);
        for unquoted in [
            "password = 'kq7v2m9xw4zt'",
            "password = `kq7v2m9xw4zt`",
            "password = kq7v2m9xw4zt",
        ] {
            assert!(!rule.regex.is_match(unquoted.as_bytes()), "{unquoted:?}");
        }
        let report = evaluate(
            &rules,
            ScanProfile::Comprehensive,
            ScanLimits::default(),
            [0u8; 32],
            quoted,
        )
        .unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.rule_id == "hashicorp-tf-password"),
            "{:?}",
            report.findings
        );
    }

    // Case-insensitive duplicate needles at one offset make scalar work stop at the lowest matching pattern index, not the number of reported matches.
    #[test]
    fn prepared_case_insensitive_any_replays_scalar_work() {
        let distinct = ["placeholder", "example", "changeme"];
        let distinct_haystacks = [
            b"ordinary text".as_slice(),
            b"EXAMPLE value".as_slice(),
            b"prefix ChangeMe".as_slice(),
        ];
        let case_duplicates = ["placeholder", "PLACEHOLDER", "example", "EXAMPLE"];
        let case_duplicate_haystacks = [
            b"ordinary text".as_slice(),
            b"placeholder value".as_slice(),
            b"PLACEHOLDER value".as_slice(),
            b"PlaceHolder value".as_slice(),
            b"an example and a PLACEHOLDER".as_slice(),
            b"an EXAMPLE only".as_slice(),
        ];
        for (needles, haystacks) in [
            (distinct.as_slice(), distinct_haystacks.as_slice()),
            (
                case_duplicates.as_slice(),
                case_duplicate_haystacks.as_slice(),
            ),
        ] {
            let needles: Vec<String> = needles.iter().map(|needle| (*needle).to_owned()).collect();
            assert_prepared_replays_scalar_work(&needles, haystacks);
        }
    }

    fn assert_prepared_replays_scalar_work(needles: &[String], haystacks: &[&[u8]]) {
        let matcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(needles)
            .unwrap();
        for haystack in haystacks {
            for max_work_bytes in 0..=haystack.len() * needles.len() + 1 {
                let limits = ScanLimits {
                    max_work_bytes,
                    ..ScanLimits::default()
                };
                let mut scalar_work = 0;
                let scalar = contains_any_charged_ignore_case_scalar(
                    haystack,
                    needles,
                    &mut scalar_work,
                    limits,
                );
                let mut prepared_work = 0;
                let prepared = contains_any_charged_ignore_case(
                    haystack,
                    needles,
                    Some(&matcher),
                    &mut prepared_work,
                    limits,
                );
                assert_eq!(scalar_work, prepared_work);
                assert!(prepared_work <= max_work_bytes || prepared.is_err());
                assert_eq!(scalar.is_ok(), prepared.is_ok());
                if let (Ok(scalar), Ok(prepared)) = (scalar, prepared) {
                    assert_eq!(scalar, prepared);
                }
            }
        }
    }

    fn only_candidate(rule: &Rule, input: &str) -> Result<Option<Finding>, Abort> {
        let captures = rule.regex.captures(input.as_bytes()).unwrap();
        let mut work = 0usize;
        evaluate_candidate(
            &EMBEDDED_RULES_FOR_TESTS,
            rule,
            &captures,
            input,
            &mut work,
            ScanLimits::default(),
        )
    }

    /// Each rule declares one group that only one alternation branch binds;
    /// the branch without it yields no finding and the branch with it does.
    #[test]
    fn a_declared_group_that_does_not_participate_skips_the_candidate() {
        for (declaration, skipped, reported) in [
            (
                r#"{"name":"t-value","regex":"(?:alpha=(?P<value>[A-Za-z0-9]{20})|beta)","anchors":["alpha"],"radius":16,"value_group":"value"}"#,
                "beta",
                "alpha=Ab3fGh1jKlMnOpQrStUv",
            ),
            (
                r#"{"name":"t-secret","regex":"(?:alpha=([A-Za-z0-9]{20})|beta=[A-Za-z0-9]{20})","anchors":["alpha"],"radius":16,"secret_group":1}"#,
                "beta=Ab3fGh1jKlMnOpQrStUv",
                "alpha=Ab3fGh1jKlMnOpQrStUv",
            ),
            (
                r#"{"name":"t-key","regex":"(?:(?P<key>[a-z_]*token)=[A-Za-z0-9]{20}|beta=[A-Za-z0-9]{20})","anchors":["token"],"radius":16,"key_group":"key"}"#,
                "beta=Ab3fGh1jKlMnOpQrStUv",
                "auth_token=Ab3fGh1jKlMnOpQrStUv",
            ),
        ] {
            let rule = alternation_rule(declaration);
            let name = rule.declaration.name.as_str();
            assert!(
                matches!(only_candidate(&rule, skipped), Ok(None)),
                "{name} reported {skipped:?}"
            );
            assert!(
                matches!(only_candidate(&rule, reported), Ok(Some(_))),
                "{name} skipped {reported:?}"
            );
        }
    }

    #[test]
    fn a_full_match_longer_than_the_match_bound_is_skipped() {
        let rule = alternation_rule(
            r#"{"name":"t-long","regex":"alpha=(?P<value>[A-Za-z0-9]{20,})","anchors":["alpha"],"radius":16,"value_group":"value"}"#,
        );
        let fits = format!("alpha={}", "Ab3fGh1jKl".repeat((MAX_MATCH_BYTES - 6) / 10));
        assert!(fits.len() <= MAX_MATCH_BYTES);
        assert!(matches!(only_candidate(&rule, &fits), Ok(Some(_))));

        let too_long = format!("alpha={}", "Ab3fGh1jKl".repeat(MAX_MATCH_BYTES / 10 + 1));
        assert!(too_long.len() > MAX_MATCH_BYTES);
        assert!(matches!(
            only_candidate(&rule, &too_long),
            Err(Abort::Match)
        ));
    }

    #[test]
    fn placeholder_shapes_are_closed() {
        for value in [
            // A plain word, with the closing punctuation a synopsis or a sentence adds.
            "PASSWORD",
            "string",
            "value",
            "my-password",
            "API_KEY]",
            "value.",
            "two words",
            "value\\n",
            "\u{2581}our",
            "\u{9700}\u{8981}\u{5bc6}\u{7801}",
            // A mask.
            "****",
            "gho_************",
            "...",
            // A reference.
            "$DB_PASSWORD",
            "os.environ/OPENAI_API_KEY",
            "env:DB_PASSWORD",
            "GITHUB_TOKEN",
            // A template.
            "<password>",
            "{password}",
            "[REDACTED]",
            "%s",
            // A file path or a URL.
            "~/secrets/token7.txt",
            "./token7",
            "../token7",
            "/path/to/secrets7.txt",
            "https://example.org/settings/tokens7",
            "%DB_PASSWORD%",
            "%AppData%",
            // A port and a property list.
            "1433;database=sales",
            // An attribute list.
            "id=npm7,src=.npmrc",
            "",
            "  ",
        ] {
            assert!(is_placeholder_shape(value.as_bytes()), "{value:?}");
        }
        for value in [
            "hunter2",
            "Summer!",
            "Strings!",
            "String!",
            "12345678",
            "12.05.1987",
            "8080:80",
            "%40abc",
            "%7qW9%",
            "%40ab9%",
            "p@ss",
            "S3cret",
            "a.b9",
            "x*y*z9",
            "ab**c1",
            "a$b1",
            "%Tgb5",
            "%7",
            "Zq7Lm2Xv9Rt4Kp8Wn3Bc6Yd1",
            // Base64 that begins with `/` or ends in padding, and an upper-case name outside the secret vocabulary.
            "/aB3dEf/Gh1jKl+mN",
            "QxWvErTy4=",
            "dXNlcg==",
            "os.environ/BUILD_ID7",
            "abcDEF_TOKEN7",
            "x9GITHUB_TOKEN",
            "/run/secrets/db7",
            "ID=npm7",
            "\u{43f}\u{430}\u{440}\u{43e}\u{43b}\u{44c}7",
        ] {
            assert!(!is_placeholder_shape(value.as_bytes()), "{value:?}");
        }
    }

    #[test]
    fn a_pin_needs_four_digits_under_a_key_that_ends_in_a_memorized_secret_word() {
        for (value, expected) in [
            ("1234", true),
            (" 123456 ", true),
            ("123", false),
            ("1234.5", false),
            ("12_34", false),
            ("0x1234", false),
            ("-1234", false),
            ("1234s", false),
        ] {
            assert_eq!(is_pin(value.as_bytes()), expected, "{value:?}");
        }
        for (key, expected) in [
            ("password", true),
            ("db_password", true),
            ("dbPassword", true),
            ("PGPASSWORD", true),
            ("client_secret", true),
            ("SECRET", true),
            ("password_length", false),
            ("secret_version", false),
            ("passwords", false),
            ("secrets", false),
            ("token", false),
            ("api_key", false),
            ("secret_key", false),
        ] {
            assert_eq!(
                ends_in_memorized_secret_word(key.as_bytes()),
                expected,
                "{key}"
            );
        }
    }

    #[test]
    fn a_token_that_ends_in_password_is_a_secret_key_token() {
        for key in [
            "PGPASSWORD",
            "pgpassword",
            "keystorepassword",
            "MyDbPassword",
        ] {
            assert!(has_secret_key_token(key.as_bytes()), "{key}");
        }
        for key in [
            "passwordless",
            "pgpasswords",
            "password_less_mode_x",
            "PGPASS",
        ] {
            assert_eq!(
                has_secret_key_token(key.as_bytes()),
                key.contains('_'),
                "{key}"
            );
        }
        // A public marker still excludes the key.
        assert!(!has_secret_key_token(b"public_pgpassword"));
    }

    #[test]
    fn basic_credentials_decode_to_user_and_password() {
        for (encoded, decoded) in [
            ("dTpw", Some(&b"u:p"[..])),
            ("dXNlcjpwYXNz", Some(b"user:pass")),
            ("dXNlcg==", Some(b"user")),
            ("dXNlcg", Some(b"user")),
            ("-_-_", Some(&[0xfb, 0xff, 0xbf][..])),
            ("+/+/", Some(&[0xfb, 0xff, 0xbf][..])),
            ("dXNlc", None),
            ("dX Nl", None),
            ("", Some(b"")),
        ] {
            assert_eq!(
                decode_base64(encoded.as_bytes()).as_deref(),
                decoded,
                "{encoded:?}"
            );
        }
        for (encoded, valid) in [
            ("dTpw", true),
            (concat!("YWxpY2U6", "aHVudGVyMg=="), true),
            // `user` lacks the `:`.
            ("dXNlcg==", false),
            // A word of prose decodes to bytes outside UTF-8 text.
            ("authentication", false),
            ("credentials", false),
            // `u:caf` and the Latin-1 byte 0xE9.
            ("dTpjYWbp", true),
            // `alice:hunter2` and the line break `echo` adds, as LF and as CRLF.
            (concat!("YWxpY2U6", "aHVudGVyMgo="), true),
            (concat!("YWxpY2U6", "aHVudGVyMg0K"), true),
            // `user` and a line break.
            ("dXNlcgo=", false),
            // `a:` and a control character.
            ("YToB", false),
            ("dXNlc", false),
        ] {
            let verdict = validate_basic_credentials(encoded.as_bytes());
            assert_eq!(
                matches!(verdict, OfflineVerdict::Valid),
                valid,
                "{encoded:?}"
            );
            assert_eq!(
                matches!(verdict, OfflineVerdict::Invalid),
                !valid,
                "{encoded:?}"
            );
        }
    }

    fn assignment_spans(input: &str) -> CandidateSpans {
        let (candidates, _) = fast_path_candidates(KeyedForm::Assignment, input);
        assert_eq!(candidates.len(), 1, "{input:?}");
        candidates[0]
    }

    /// The reported span runs through `;` to the end of the line, the suppressor value stays
    /// the text before the first `;`, and the walk charges its whole bounded window up front.
    #[test]
    fn a_semicolon_continuation_runs_to_the_end_of_its_line_and_is_charged() {
        for (input, reported) in [
            ("password=ab;cd", "ab;cd"),
            ("password=ab;cd;ef\nnext", "ab;cd;ef"),
            ("password=ab;cd\r\n", "ab;cd"),
            ("password=ab;cd \t\nnext", "ab;cd"),
            ("password=ab;cd\\nnext", "ab;cd"),
            ("password=ab;cd  \\r\\nnext", "ab;cd"),
            ("password=ab;c\u{e9}", "ab;c\u{e9}"),
            ("password=ab;c\\ d", "ab;c\\ d"),
            // Punctuation after the joined text closes a statement and stays outside the span.
            ("password=ab;cd;", "ab;cd"),
            ("password=ab;cd;}\nnext", "ab;cd"),
            ("password=ab;cd;; # note", "ab;cd"),
            ("password=ab;cd; echo ok", "ab"),
            ("password=ab;cd;;ef", "ab"),
            ("password=ab;cd;)", "ab"),
            // A pair, a following word, a separator of any script, or a bare `;` at the line's end.
            ("password=ab;cd=ef", "ab"),
            ("password=ab;cd;ef=gh", "ab"),
            ("password=ab;cd ef", "ab"),
            ("password=ab;cd\u{a0}ef", "ab"),
            ("password=ab;cd\"", "ab"),
            ("password=ab;", "ab"),
            ("password=ab;\\n", "ab"),
            ("password=ab; cd", "ab"),
            ("password=ab;;cd", "ab"),
        ] {
            let spans = assignment_spans(input);
            let mut work = 0usize;
            let continued =
                continued_past_semicolons(input, spans, &mut work, ScanLimits::default()).unwrap();
            assert_eq!(
                &input[continued.reported.start()..continued.reported.end()],
                reported,
                "{input:?}"
            );
            assert_eq!(continued.full.end(), continued.reported.end(), "{input:?}");
            assert_eq!(continued.value, spans.value, "{input:?}");
            assert_eq!(continued.key, spans.key, "{input:?}");
        }

        // The window is charged before the walk reads it, and a budget short of it stops the walk.
        let input = format!(
            "password=ab;{}\n{}",
            "x".repeat(64),
            "y".repeat(4 * MAX_CONTINUATION_BYTES)
        );
        let spans = assignment_spans(&input);
        let mut work = 0usize;
        continued_past_semicolons(&input, spans, &mut work, ScanLimits::default()).unwrap();
        assert_eq!(work, MAX_CONTINUATION_BYTES);
        let mut work = 0usize;
        let tight = ScanLimits {
            max_work_bytes: MAX_CONTINUATION_BYTES - 1,
            ..ScanLimits::default()
        };
        assert!(matches!(
            continued_past_semicolons(&input, spans, &mut work, tight),
            Err(Abort::Work)
        ));
        // A value with nothing joined to it charges nothing.
        let spans = assignment_spans("password=ab cd");
        let mut work = 0usize;
        continued_past_semicolons("password=ab cd", spans, &mut work, ScanLimits::default())
            .unwrap();
        assert_eq!(work, 0);
        // A closing bracket after `;`, a `#` comment after blanks, and a canonical escape.
        for (input, reported) in [
            ("useAuth=false;}", "false"),
            ("password=ab;cd # note", "ab;cd"),
            ("password=ab;cd#note", "ab;cd#note"),
            ("password=ab;cd\\u000anext", "ab;cd"),
        ] {
            let spans = assignment_spans(input);
            let mut work = 0usize;
            let continued =
                continued_past_semicolons(input, spans, &mut work, ScanLimits::default()).unwrap();
            assert_eq!(
                &input[continued.reported.start()..continued.reported.end()],
                reported,
                "{input:?}"
            );
        }
    }

    /// The walk reads a fixed span past the value, so its verdict is the same wherever the
    /// text is cut beyond that span. A tail that ends inside the span is joined, a tail that a
    /// following word disproves inside it leaves the value as matched, and a tail still open
    /// at the bound fails the scan closed.
    #[test]
    fn a_semicolon_continuation_reads_a_bounded_span_past_the_value() {
        let rules = RuleSet::from_embedded().unwrap();
        let scan = |input: &str| {
            let report = evaluate(
                &rules,
                ScanProfile::Conservative,
                ScanLimits::default(),
                [0u8; 32],
                input,
            )
            .unwrap();
            let lengths = report
                .findings
                .iter()
                .map(|finding| finding.value_span.len())
                .collect::<Vec<_>>();
            (lengths, report.limits_hit)
        };
        let inside = "x".repeat(MAX_CONTINUATION_BYTES - 2);
        assert_eq!(
            scan(&format!("password=ab7;{inside}\nnext")),
            (vec!["ab7;".len() + inside.len()], None)
        );
        assert_eq!(
            scan(&format!("password=ab7;tail word {inside}{inside}")),
            (vec!["ab7".len()], None)
        );
        for tail in [MAX_CONTINUATION_BYTES, 2 * MAX_MATCH_BYTES] {
            let open = "x".repeat(tail);
            for input in [
                format!("password=ab7;{open}"),
                format!("password=ab7;{open} next"),
                format!("password=12;{open}\n"),
            ] {
                assert_eq!(scan(&input), (vec![], Some(LimitExhausted::Match)));
            }
            // A key the gate rejects leaves its tail unread.
            assert_eq!(scan(&format!("monkey=ab7;{open}")), (vec![], None));
        }
    }

    /// The scalar and code-reference gates read the joined value, and the suppressors read
    /// its head.
    #[test]
    fn the_shape_gates_read_a_continued_value_whole() {
        let rules = RuleSet::from_embedded().unwrap();
        let values = |input: &'static str| {
            let report = evaluate(
                &rules,
                ScanProfile::Conservative,
                ScanLimits::default(),
                [0u8; 32],
                input,
            )
            .unwrap();
            rule_values(input, &report.findings)
        };
        assert_eq!(values("PASSWORD=12;a7B"), ["12;a7B"]);
        // A member-path head, joined at run time as the other credential shapes are.
        let dotted: &'static str = ["password=", "my.", "pass;w0rd"].concat().leak();
        assert_eq!(values(dotted), [&dotted["password=".len()..]]);
        assert_eq!(
            values("password=hunter2;placeholder"),
            ["hunter2;placeholder"]
        );
        // Without a continuation the gates read the value alone.
        assert_eq!(values("PASSWORD=12; echo done"), Vec::<&str>::new());
        let statement: &'static str = ["password=", "my.", "pass; echo"].concat().leak();
        assert_eq!(values(statement), Vec::<&str>::new());
        assert_eq!(values("password=placeholder;x9"), Vec::<&str>::new());
    }

    fn rule_values<'a>(input: &'a str, findings: &[Finding]) -> Vec<&'a str> {
        findings
            .iter()
            .map(|finding| &input[finding.value_span.start()..finding.value_span.end()])
            .collect()
    }

    /// The line gate reports a value by the text after it: the end of its line or a delimiter
    /// directly after it reports the value, and blanks before another word leave it.
    #[test]
    fn the_line_gate_reads_the_text_after_a_value() {
        for (rest, after_assignment, after_field) in [
            ("", true, true),
            ("\n", true, true),
            ("\r\n", true, true),
            (" \t\n", true, true),
            ("  # note", true, true),
            ("\\n", true, true),
            ("\\u000a", true, true),
            (" \\\n", true, true),
            ("\"", true, true),
            ("'", true, true),
            ("\" next", true, true),
            ("\\\"}", true, true),
            // A quote before a letter stands inside a word; the assignment rule ends its run there.
            ("'word9", true, false),
            ("\\word9", false, false),
            ("\\\"word9", false, false),
            (")", true, false),
            (";", true, false),
            ("|cat", true, false),
            ("&x", true, false),
            // Blanks, then the delimiter that closes the text around the value.
            (" \"", true, false),
            (" )", true, false),
            (" ;", true, false),
            (" | cat", true, false),
            (" <", true, false),
            ("\u{2028}next", true, true),
            ("\x0Cnext", true, true),
            (" \\\\\\n  --flag", true, true),
            ("\"}", true, true),
            ("'.cd9", true, false),
            ("\"),next", true, false),
            (" \"\u{a0}next", true, false),
            // Blanks, then another word.
            (" next", false, false),
            ("\t--flag", false, false),
            (" \"quoted\"", false, false),
            ("\u{a0}next", false, false),
        ] {
            let input = format!("v{rest}");
            for (tail, expected) in [
                (LineTail::Delimiter, after_assignment),
                (LineTail::ClosingQuote, after_field),
            ] {
                let mut work = 0;
                let ends = value_ends_its_line(&input, 1, tail, &mut work, ScanLimits::default());
                assert_eq!(ends.unwrap(), expected, "{rest:?} {tail:?}");
                assert_eq!(work, rest.len(), "{rest:?}");
            }
        }
    }

    /// The gate reads and charges `MAX_CONTINUATION_BYTES` past the value. A run of blanks
    /// that reaches the bound counts as the end of the line, in every scan window alike.
    #[test]
    fn the_line_gate_reads_a_bounded_span_past_the_value() {
        let input = format!("v{}next", " ".repeat(2 * MAX_CONTINUATION_BYTES));
        let mut work = 0;
        let ends = value_ends_its_line(
            &input,
            1,
            LineTail::ClosingQuote,
            &mut work,
            ScanLimits::default(),
        );
        assert!(ends.unwrap());
        assert_eq!(work, MAX_CONTINUATION_BYTES);
        let limits = ScanLimits {
            max_work_bytes: MAX_CONTINUATION_BYTES - 1,
            ..ScanLimits::default()
        };
        let mut work = 0;
        assert!(matches!(
            value_ends_its_line(&input, 1, LineTail::ClosingQuote, &mut work, limits),
            Err(Abort::Work)
        ));
    }

    /// A word that is an assignment of its own, a comment, a bracket, or a rule of
    /// punctuation is structure; Base64 padding ends a value.
    #[test]
    fn a_detached_word_that_is_an_assignment_is_structure() {
        for word in [
            "DATABASE_URL=postgres://localhost/db",
            "URL=",
            "api.key=x",
            "_a=1",
            "a=b=c",
            "db.password=",
            "x-api-key=",
            "FOO=;bar",
            "#comment7",
            "}",
            "[section]",
            "{a}",
            "---",
        ] {
            assert!(is_structural_line(word.as_bytes()), "{word}");
        }
        for word in [
            "QUJDRDEyMzQ1Ng==",
            "q7Lm-2Xv_9Rt4Kp=",
            "p@ss=w0rd",
            "Xy7Kp2Lm9Qa=;tail9",
            "clientSecret=",
            "9a=b",
            "hunter-two",
            "Url=",
            "=x",
        ] {
            assert!(!is_structural_line(word.as_bytes()), "{word}");
        }
    }

    /// A detached value ends where its run writes a line break as an escape, and an escape
    /// that opens the run stays in it.
    #[test]
    fn a_detached_value_ends_at_an_escaped_line_break() {
        let rules = RuleSet::from_embedded().unwrap();
        for (input, value) in [
            ("api_token= hunter2\\nnext line", "hunter2"),
            ("api_token=\nhunter2\\r\\nnext line", "hunter2"),
            ("api_token= hunter2\\u000anext line", "hunter2"),
            ("api_token= \\nhunter2", "\\nhunter2"),
            ("api_token= \\nhunter2\\nnext line", "\\nhunter2"),
            // Another escape pair is value text.
            ("api_token= hunter\\x2", "hunter\\x2"),
        ] {
            let report = evaluate(
                &rules,
                ScanProfile::Conservative,
                ScanLimits::default(),
                [0u8; 32],
                input,
            )
            .unwrap();
            assert_eq!(rule_values(input, &report.findings), [value], "{input:?}");
            assert_eq!(report.findings[0].rule_id, "magic-detached-assignment");
        }
    }

    /// A rejected candidate of a line-shaped rule resumes the search at its value, so a key
    /// held there is read as a candidate of its own.
    #[test]
    fn a_rejected_line_shaped_candidate_resumes_the_search_at_its_value() {
        let rules = RuleSet::from_embedded().unwrap();
        for (input, rule_id, value) in [
            (
                "API_TOKEN=\nDB_PASSWORD= hunter2",
                "magic-detached-assignment",
                "hunter2",
            ),
            (
                "a_key= b_key= c_key= hunter2",
                "magic-detached-assignment",
                "hunter2",
            ),
            ("secret: password: hunter2", "magic-field-value", "hunter2"),
        ] {
            let report = evaluate(
                &rules,
                ScanProfile::Conservative,
                ScanLimits::default(),
                [0u8; 32],
                input,
            )
            .unwrap();
            assert!(report.is_complete());
            assert_eq!(rule_values(input, &report.findings), [value], "{input:?}");
            assert_eq!(report.findings[0].rule_id, rule_id, "{input:?}");
        }
        // A chain of empty assignments costs one candidate per key.
        let chain = "a_key= ".repeat(1000);
        let report = evaluate(
            &rules,
            ScanProfile::Conservative,
            ScanLimits::default(),
            [0u8; 32],
            &chain,
        )
        .unwrap();
        assert!(report.is_complete());
        assert!(report.findings.is_empty());
        assert!(report.candidates_evaluated <= 1000 * 2);
    }

    /// The assignment rule resumes inside the value of a key that stands after an empty
    /// assignment, on both of its paths: the hand-coded one for ASCII text and the pattern
    /// for text of another script.
    #[test]
    fn the_assignment_rule_resumes_inside_a_value_on_both_paths() {
        let rules = RuleSet::from_embedded().unwrap();
        for (input, values) in [
            (
                "db.password =\nkeycloak.url = https://sso.internal/auth?client_secret=hunter2",
                vec!["hunter2"],
            ),
            (
                "API_TOKEN=\nself.token = api_token = hunter2",
                vec!["api_token", "hunter2"],
            ),
            (
                "API_TOKEN= v1x9\\nDB_PASSWORD= hunter2",
                vec!["v1x9", "hunter2"],
            ),
            // Standing alone, a value under a key of another kind is one candidate.
            (
                "keycloak.url = https://sso.internal/auth?client_secret=hunter2",
                vec![],
            ),
        ] {
            for input in [input.to_owned(), format!("\u{e9} {input}")] {
                let report = evaluate(
                    &rules,
                    ScanProfile::Conservative,
                    ScanLimits::default(),
                    [0u8; 32],
                    &input,
                )
                .unwrap();
                assert!(report.is_complete(), "{input:?}");
                assert_eq!(rule_values(&input, &report.findings), values, "{input:?}");
            }
        }
    }

    /// A resumed search reads the value of one candidate again, and only a key near the start
    /// of the run after an empty assignment resumes, so a long run of `name=` pairs below one
    /// costs a bounded number of candidates.
    #[test]
    fn a_resumed_search_reads_one_value_again() {
        let rules = RuleSet::from_embedded().unwrap();
        let line = format!("a_key=\n{}\n", "monkey=".repeat(4000));
        let ascii = line.repeat(12);
        for input in [ascii.clone(), format!("\u{e9}\n{ascii}")] {
            let report = evaluate(
                &rules,
                ScanProfile::Conservative,
                ScanLimits::default(),
                [0u8; 32],
                &input,
            )
            .unwrap();
            assert!(report.is_complete());
            assert!(report.findings.is_empty());
            // A key within the first 64 bytes of the run resumes; the others pass in one read.
            assert!(report.candidates_evaluated <= 12 * 16);
            assert!(report.work_bytes <= 32 * input.len());
        }
    }

    /// At the bound of its walk the continuation reads blanks as the end of the line and
    /// fails the scan closed on text that is still open.
    #[test]
    fn a_continuation_at_its_bound_ends_the_line_or_fails_closed() {
        let blanks = " ".repeat(2 * MAX_CONTINUATION_BYTES);
        let input = format!("password=ab;cd{blanks}next");
        let mut work = 0usize;
        let continued = continued_past_semicolons(
            &input,
            assignment_spans(&input),
            &mut work,
            ScanLimits::default(),
        )
        .unwrap();
        assert_eq!(
            &input[continued.reported.start()..continued.reported.end()],
            "ab;cd"
        );
        let closers = "}".repeat(2 * MAX_CONTINUATION_BYTES);
        for input in [
            format!("password=ab;{closers}Z9"),
            format!("password=ab;cd;{closers}Z9"),
        ] {
            let mut work = 0usize;
            assert!(matches!(
                continued_past_semicolons(
                    &input,
                    assignment_spans(&input),
                    &mut work,
                    ScanLimits::default()
                ),
                Err(Abort::Match)
            ));
        }
    }

    /// Serialized text writes a line break as `\n`, and the key pattern reads the letter as
    /// the key's first character. The key gate reads the name after it.
    #[test]
    fn a_key_after_an_escaped_line_break_is_read_without_the_escape_letter() {
        let rules = RuleSet::from_embedded().unwrap();
        for (input, key, value) in [
            ("first line\\ntoken=hunter2", "token", "hunter2"),
            ("a\\r\\napi_key = hunter2 b", "api_key", "hunter2"),
            ("x\\tsecret=hunter2", "secret", "hunter2"),
            ("x\\u000atoken=hunter2", "token", "hunter2"),
            // A name that is a key as written keeps its first letter.
            ("C:\\token=hunter2", "token", "hunter2"),
            ("\\npassword=hunter2", "password", "hunter2"),
        ] {
            let report = evaluate(
                &rules,
                ScanProfile::Conservative,
                ScanLimits::default(),
                [0u8; 32],
                input,
            )
            .unwrap();
            assert_eq!(rule_values(input, &report.findings), [value], "{input:?}");
            let key_span = report.findings[0].key_span.unwrap();
            assert_eq!(&input[key_span.start()..key_span.end()], key, "{input:?}");
        }
        for input in [
            "a\\nmonkey=hunter2",
            "a\\xtoken=hunter2",
            "ntoken=hunter2",
            "a\\npublic_key=hunter2",
            "a\\tpubkey=hunter2",
        ] {
            let report = evaluate(
                &rules,
                ScanProfile::Conservative,
                ScanLimits::default(),
                [0u8; 32],
                input,
            )
            .unwrap();
            assert_eq!(report.findings, [], "{input:?}");
        }
    }

    /// The key gate walks every byte of the key, so the key is charged to
    /// `max_work_bytes` before the gate runs, for a rejected key as well as
    /// an accepted one.
    #[test]
    fn the_key_gate_charges_the_key_to_the_work_budget() {
        let rule = alternation_rule(
            r#"{"name":"t-key-work","regex":"(?P<key>[a-z_]+)=(?P<value>[A-Za-z0-9]{20})","anchors":["="],"radius":16,"key_group":"key","value_group":"value"}"#,
        );
        let rules = RuleSet::from_embedded().unwrap();
        for key in ["a".repeat(500), format!("{}_token", "a".repeat(494))] {
            let input = format!("{key}=Ab3fGh1jKlMnOpQrStUv");
            let captures = rule.regex.captures(input.as_bytes()).unwrap();

            let mut work = 0usize;
            let outcome = evaluate_candidate(
                &rules,
                &rule,
                &captures,
                &input,
                &mut work,
                ScanLimits::default(),
            );
            assert!(outcome.is_ok(), "{key}: {outcome:?}");
            assert!(
                work >= key.len(),
                "{key}: charged {work} for a {} byte key",
                key.len()
            );

            let mut work = 0usize;
            let outcome = evaluate_candidate(
                &rules,
                &rule,
                &captures,
                &input,
                &mut work,
                ScanLimits {
                    max_work_bytes: key.len() - 1,
                    ..ScanLimits::default()
                },
            );
            assert!(matches!(outcome, Err(Abort::Work)), "{key}: {outcome:?}");
        }
    }

    const SLACK_DIGIT_PREFIXES: [&str; 8] = [
        "xoxb-",
        "xoxp-",
        "xoxo-",
        "xoxs-",
        "xoxe-",
        "XOXE-",
        "xoxe.xoxb-",
        "XOXE.XOXP-",
    ];

    const SLACK_BODY_PIECES: [&str; 10] =
        ["0", "1", "12", "9876543210", "-", "-", "a", "Z", "x", "AbC"];

    /// `validate_slack` must agree with this split-and-count reading for the prefixes whose shape fixes leading all-digit segments.
    fn slack_digit_shape_oracle(prefix: &str, body: &[u8]) -> Option<bool> {
        if prefix.len() + body.len() < 10 {
            return None;
        }
        let segments: Vec<&[u8]> = body.split(|byte| *byte == b'-').collect();
        let fixed = match prefix {
            "xoxb-" | "xoxe.xoxb-" | "XOXE.XOXP-" => 1,
            "xoxe-" | "XOXE-" if segments.len() == 2 => 1,
            _ => 3,
        };
        Some(
            segments.len() > fixed
                && segments[..fixed]
                    .iter()
                    .all(|segment| !segment.is_empty() && segment.iter().all(u8::is_ascii_digit)),
        )
    }

    fn assert_slack_digit_shape_matches_oracle(prefix: &str, body: &str) {
        let value = format!("{prefix}{body}");
        let observed = match validate_slack(value.as_bytes()) {
            OfflineVerdict::Valid => Some(true),
            OfflineVerdict::Invalid => Some(false),
            OfflineVerdict::Indeterminate => None,
        };
        assert_eq!(
            observed,
            slack_digit_shape_oracle(prefix, body.as_bytes()),
            "{value}"
        );
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(2_000))]
        #[test]
        fn slack_digit_shapes_match_the_segment_count_reading(
            prefix in 0..SLACK_DIGIT_PREFIXES.len(),
            pieces in proptest::collection::vec(0..SLACK_BODY_PIECES.len(), 0..16)
        ) {
            let body: String = pieces.iter().map(|&piece| SLACK_BODY_PIECES[piece]).collect();
            assert_slack_digit_shape_matches_oracle(SLACK_DIGIT_PREFIXES[prefix], &body);
        }
    }

    #[test]
    fn slack_digit_shapes_match_the_segment_count_reading_on_rule_shapes() {
        let refresh: String = "ABCDEF0123456789".repeat(10).chars().take(146).collect();
        for prefix in SLACK_DIGIT_PREFIXES {
            for body in [
                "1234567890-1234567890-1234567890-AbCdEfGhIjKlMn-OpQrStUvWxYz1234",
                "1234567890-1234567890-Ab-Cd-efgh",
                "1234567890-1234567890-1234567890-",
                "1-22-333-abcdef123456",
                "1-22-333",
                "1-22-333-",
                "1-",
                "1",
                "-1-2-3-4",
                "1--2-3-4",
                "12a-34-56-tail",
                &format!("1-{refresh}"),
                &format!("1-{refresh}-tail"),
                "",
            ] {
                assert_slack_digit_shape_matches_oracle(prefix, body);
            }
        }
    }
}

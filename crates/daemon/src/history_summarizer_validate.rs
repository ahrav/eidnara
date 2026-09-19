//! The module validates history_summarizer history_segment XML before publication.
//! Validation compares the XML against the raw chunk and persisted history_segment ranges.
//! Validation completes before any side effect publishes output.
//!
//! The functions in this module are pure.
//! Validation rejects malformed ranges, stale chunks, and invalid message-id endpoints before database writes.
//! Validation resolves boundary-healing decisions before database writes.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::boundary::completed_tool_arc_crosses_boundary;
use crate::history_summarizer_citations::{
    Citation, ExtractionFailure, ExtractionOutcome, FrozenAliasTable, check_fact_set,
    split_citations,
};

const BOUNDARY_HEALING_SLACK: u64 = 2;

/// MessageRange includes both endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageRange {
    pub start: u64,
    pub end: u64,
}

/// ChunkLine maps a formatted chunk line to a provider message ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkLine {
    pub ordinal: u64,
    /// message_id is the last flat block's `<mid>#<index>` ID.
    /// message_id is empty only when the raw message has no flat blocks.
    pub message_id: String,
    /// anchorable is true only when message_id names a real flat block.
    /// A history_segment must end on an anchorable block to avoid impossible coverage boundaries.
    #[serde(default = "chunk_line_anchorable_by_default")]
    pub anchorable: bool,
}

fn chunk_line_anchorable_by_default() -> bool {
    true
}

/// HistorySummarizerChunk identifies the raw-history slice submitted to the history_summarizer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistorySummarizerChunk {
    pub start_index: u64,
    pub end_index: u64,
    pub lines: Vec<ChunkLine>,
    /// The frozen aliases the rendered text carries, so a citation resolves to native identity and exact presented bytes.
    #[serde(default)]
    pub aliases: FrozenAliasTable,
    /// present_ordinals contains all non-synthetic input ordinals visible when the chunk was built.
    /// present_ordinals may be sparse when message identities are re-minted.
    /// Validation filters present_ordinals to the claimed range.
    /// Validation must not assume that present_ordinals has dense `0..n` coverage.
    #[serde(default)]
    pub present_ordinals: Vec<u64>,
    /// Gaps wholly within tool_only_ranges may be healed at any size.
    /// tool_only_ranges may contain only tool-only transcript noise.
    #[serde(default)]
    pub tool_only_ranges: Vec<MessageRange>,
    /// completed_tool_arcs identifies invocation/result ranges whose terminal publication boundary must remain atomic.
    #[serde(default)]
    pub completed_tool_arcs: Vec<MessageRange>,
}

/// StoredHistorySegmentRange records an already-persisted history_segment's raw ordinal range.
/// Validation uses these ordinals to verify store ordering before appending history_segments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredHistorySegmentRange {
    pub start_message: u64,
    pub end_message: u64,
}

/// ValidateOptions contains runner-supplied options absent from history_summarizer XML.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidateOptions {
    /// sequence_offset assigns the sequence number of the first emitted history_segment.
    #[serde(default)]
    pub sequence_offset: u64,
    /// When in_emergency is true, recovery favors rapid raw-history reduction over the newest history_segment's final boundary quality.
    #[serde(default)]
    pub in_emergency: bool,
    #[serde(default = "default_true")]
    pub memory_enabled: bool,
    #[serde(default = "default_true")]
    pub auto_promote: bool,
    #[serde(default)]
    pub user_memory_collection_enabled: bool,
    /// Explicit wrapup runs retain their final history_segment instead of deleting it during cleanup.
    #[serde(default)]
    pub force_keep_last_history_segment: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ValidateOptions {
    fn default() -> Self {
        Self {
            sequence_offset: 0,
            in_emergency: false,
            memory_enabled: true,
            auto_promote: true,
            user_memory_collection_enabled: false,
            force_keep_last_history_segment: false,
        }
    }
}

/// The struct stores a parsed history_segment before validation resolves endpoint IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedHistorySegment {
    pub start_message: u64,
    pub end_message: u64,
    pub title: String,
    /// In v2 history_segments, the main body is duplicated into `p1`; v1/legacy history_segments store body text only in `content`.
    pub content: String,
    #[serde(default)]
    pub p1: Option<String>,
    #[serde(default)]
    pub p2: Option<String>,
    #[serde(default)]
    pub p3: Option<String>,
    #[serde(default)]
    pub p4: Option<String>,
    #[serde(default)]
    pub importance: Option<u64>,
    #[serde(default)]
    pub episode_type: Option<String>,
}

/// A fact extracted from the `<facts>` block. A fact proves its source through its citations' native ordinals, so it carries no segment anchor; a legacy `[at_history_segment=N]` prefix is stripped and ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactCandidate {
    pub category: String,
    pub content: String,
    /// Frozen-alias citations the fact carried, checked by `check_fact_set` before the set is accepted.
    #[serde(default)]
    pub citations: Vec<Citation>,
}

/// A history_summarizer event uses its XML element name as `kind` and child element text keyed by element name as `fields`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedEvent {
    pub kind: String,
    #[serde(default)]
    pub at_history_segment: Option<u64>,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}

/// PrimerCandidate represents a durable standing question for later primer generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimerCandidate {
    pub question: String,
    /// `origin_history_segment_index` is a 1-based index into this history_summarizer output's emitted history_segments.
    #[serde(default)]
    pub origin_history_segment_index: Option<u64>,
}

/// UserObservationCandidate represents an optional user-memory observation extracted from the chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserObservationCandidate {
    pub content: String,
    /// TypeScript observations are unanchored.
    /// When boundary healing discards the last history_segment, validation skips an observation without `origin_history_segment_index` because it cannot prove the observation's source history_segment.
    #[serde(default)]
    pub origin_history_segment_index: Option<u64>,
}

/// ParsedHistorySegmentOutput stores XML-ish history_summarizer output before validation mutates or heals ranges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedHistorySegmentOutput {
    #[serde(default)]
    pub history_segments: Vec<ParsedHistorySegment>,
    #[serde(default)]
    pub facts: Vec<FactCandidate>,
    /// Whether the output carried a `<facts>` block at all, so an absent block and an empty one are told apart from a malformed item.
    #[serde(default)]
    pub facts_block_present: bool,
    /// A fact item whose citation syntax did not parse; the set is rejected for it rather than the item being dropped.
    #[serde(default)]
    pub fact_syntax_failure: Option<ExtractionFailure>,
    #[serde(default)]
    pub events: Vec<ParsedEvent>,
    #[serde(default)]
    pub unprocessed_from: Option<u64>,
    #[serde(default)]
    pub user_observations: Vec<UserObservationCandidate>,
    #[serde(default)]
    pub primer_candidates: Vec<PrimerCandidate>,
}

/// ValidatedHistorySegment stores raw endpoints resolved to provider message IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatedHistorySegment {
    pub sequence: u64,
    pub start_message: u64,
    pub end_message: u64,
    pub start_message_id: String,
    pub end_message_id: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub p1: Option<String>,
    #[serde(default)]
    pub p2: Option<String>,
    #[serde(default)]
    pub p3: Option<String>,
    #[serde(default)]
    pub p4: Option<String>,
    #[serde(default)]
    pub importance: Option<u64>,
    #[serde(default)]
    pub episode_type: Option<String>,
}

/// ValidatedChunk is the side-effect-free publish plan that validation produces.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatedChunk {
    pub history_segments: Vec<ValidatedHistorySegment>,
    /// The accepted facts; empty unless `extraction` is `Accepted`.
    pub facts: Vec<FactCandidate>,
    /// Q30: the fact set's own verdict, independent of the history's validity.
    #[serde(default)]
    pub extraction: ExtractionOutcome,
    pub events: Vec<ParsedEvent>,
    pub primer_candidates: Vec<PrimerCandidate>,
    pub user_observations: Vec<UserObservationCandidate>,
    /// `unprocessed_from` is the next raw ordinal to read after the safe-to-persist history_segments.
    pub unprocessed_from: u64,
    /// Validation withholds the provisional last history_segment when this flag is true so the next run can re-derive it with real lookahead.
    pub discarded_last: bool,
}

/// Validation failures are plain, serializable messages because callers include them in repair prompts and telemetry.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[error("{message}")]
pub struct HistorySummarizerValidationError {
    pub message: String,
}

fn validation_error(message: impl Into<String>) -> HistorySummarizerValidationError {
    HistorySummarizerValidationError {
        message: message.into(),
    }
}

/// The parser requires one complete history_summarizer-output envelope and uses the TypeScript host parser's permissive extraction semantics within that root.
/// Malformed inner XML yields fewer usable structures for validation to assess.
pub fn parse_history_segment_output(
    text: &str,
) -> Result<ParsedHistorySegmentOutput, HistorySummarizerValidationError> {
    // Models wrap the document in a Markdown fence or add a sentence after it; the one
    // complete root is still required, and everything outside it is ignored rather than fatal.
    let Some(root) = output_document_regex().captures(text) else {
        return Err(validation_error(
            "HistorySummarizer output must be one complete <output> root document.",
        ));
    };
    let root_body = root
        .name("body")
        .map(|capture| capture.as_str())
        .unwrap_or_default();
    // The root's own open and close tags are the only `output` tags allowed anywhere.
    if output_tag_regex().find_iter(text).count() != 2 {
        return Err(validation_error(
            "HistorySummarizer output must contain exactly one <output> root document.",
        ));
    }
    // Every structure below is read from the root document only.
    let text = root_body;

    let mut history_segments = Vec::new();
    let mut facts = Vec::new();

    for caps in history_segment_regex().captures_iter(text) {
        let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        let inner = caps.get(2).map(|m| m.as_str()).unwrap_or_default();

        let start_message = match capture_u64(attr_start_regex(), attrs) {
            Some(v) => v,
            None => continue,
        };
        let end_message = match capture_u64(attr_end_regex(), attrs) {
            Some(v) => v,
            None => continue,
        };
        let title = match capture_string(attr_title_regex(), attrs) {
            Some(v) if !v.is_empty() => unescape_xml(&v),
            _ => continue,
        };
        if title.is_empty() {
            continue;
        }

        let episode_type = capture_string(attr_episode_regex(), attrs).map(|s| unescape_xml(&s));
        let importance = capture_u64(attr_importance_regex(), attrs);

        let p1 = extract_tier(inner, 0);
        if let Some(p1_value) = p1.filter(|s| !s.is_empty()) {
            let p2 = extract_tier(inner, 1);
            let p3 = extract_tier(inner, 2);
            let p4 = extract_tier(inner, 3);
            let p2_value = p2.clone().unwrap_or_else(|| p1_value.clone());
            let p3_value = p3
                .clone()
                .unwrap_or_else(|| p2.clone().unwrap_or_else(|| p1_value.clone()));
            let p4_value = p4.unwrap_or_default();
            history_segments.push(ParsedHistorySegment {
                start_message,
                end_message,
                title,
                content: p1_value.clone(),
                p1: Some(p1_value),
                p2: Some(p2_value),
                p3: Some(p3_value),
                p4: Some(p4_value),
                importance,
                episode_type,
            });
            continue;
        }

        let content = unescape_xml(inner.trim());
        if !content.is_empty() {
            history_segments.push(ParsedHistorySegment {
                start_message,
                end_message,
                title,
                content,
                p1: None,
                p2: None,
                p3: None,
                p4: None,
                importance,
                episode_type,
            });
        }
    }

    let facts_block = facts_block_regex().captures(text);
    let facts_block_present = facts_block.is_some();
    let mut fact_syntax_failure = None;
    // A `<facts>` tag without its partner is a truncated or malformed block, never an unwrapped category the bare fallback may read.
    let facts_blocks = facts_block_regex().captures_iter(text).count();
    let stray_facts_tag = text.matches("<facts>").count() != facts_blocks
        || text.matches("</facts>").count() != facts_blocks;
    let facts_scope = if let Some(caps) = facts_block {
        let scope = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        // Anything inside `<facts>` that the item grammar does not read is malformed material, never an absent fact. A category whose closing tag names another category, and a second `<facts>` block, are likewise unreadable rather than ignored.
        let leftover = category_block_regex().replace_all(scope, "");
        if facts_blocks > 1
            || stray_facts_tag
            || !leftover.trim().is_empty()
            || category_block_regex().captures_iter(scope).any(|category| {
                category.get(1).map(|m| m.as_str()) != category.get(3).map(|m| m.as_str())
                    || category.get(2).is_some_and(|block| {
                        block.as_str().lines().any(|line| {
                            !line.trim().is_empty() && !line.trim_start().starts_with('*')
                        })
                    })
            })
        {
            fact_syntax_failure = Some(ExtractionFailure::MalformedFacts);
        }
        scope.to_string()
    } else {
        if stray_facts_tag {
            fact_syntax_failure = Some(ExtractionFailure::MalformedFacts);
        }
        let without_events = events_block_regex().replace_all(text, "");
        history_segment_regex()
            .replace_all(&without_events, "")
            .to_string()
    };

    for category_caps in category_block_regex().captures_iter(&facts_scope) {
        let category = category_caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        let closing = category_caps.get(3).map(|m| m.as_str()).unwrap_or_default();
        if category != closing {
            continue;
        }
        let block = category_caps.get(2).map(|m| m.as_str()).unwrap_or_default();
        for item_caps in fact_item_regex().captures_iter(block) {
            let raw = item_caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let unescaped = unescape_xml(raw.trim());
            let (_, content) = split_anchor_prefix(&unescaped);
            let (citations, content) = match split_citations(&content) {
                Ok((citations, content)) => (citations, content.to_string()),
                Err(failure) => {
                    fact_syntax_failure.get_or_insert(failure);
                    (Vec::new(), content)
                }
            };
            // An item with no text is malformed, not an absent fact: dropping it would turn a bad item into apparent no-fact success. Citations alone are a bad citation; a bare bullet is unreadable material.
            if content.is_empty() {
                fact_syntax_failure.get_or_insert(if citations.is_empty() {
                    ExtractionFailure::MalformedFacts
                } else {
                    ExtractionFailure::MalformedCitation
                });
                continue;
            }
            facts.push(FactCandidate {
                category: category.to_string(),
                content,
                citations,
            });
        }
    }

    let unprocessed_from = capture_u64(unprocessed_regex(), text);

    let mut user_observations = Vec::new();
    if let Some(caps) = user_observations_regex().captures(text) {
        let block = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        for item_caps in user_obs_item_regex().captures_iter(block) {
            let raw = item_caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let unescaped = unescape_xml(raw.trim());
            let (origin_history_segment_index, content) = split_anchor_prefix(&unescaped);
            if !content.is_empty() {
                user_observations.push(UserObservationCandidate {
                    content,
                    origin_history_segment_index,
                });
            }
        }
    }

    let mut primer_candidates = Vec::new();
    if let Some(caps) = primer_candidates_regex().captures(text) {
        let block = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        let mut saw_element = false;
        for primer_caps in primer_element_regex().captures_iter(block) {
            saw_element = true;
            let question = primer_caps
                .get(2)
                .map(|m| unescape_xml(m.as_str().trim()))
                .unwrap_or_default();
            if !question.is_empty() {
                primer_candidates.push(PrimerCandidate {
                    question,
                    origin_history_segment_index: primer_caps
                        .get(1)
                        .and_then(|m| m.as_str().parse::<u64>().ok()),
                });
            }
        }
        if !saw_element {
            for item_caps in primer_item_regex().captures_iter(block) {
                let question = item_caps
                    .get(1)
                    .map(|m| unescape_xml(m.as_str().trim()))
                    .unwrap_or_default();
                if !question.is_empty() {
                    primer_candidates.push(PrimerCandidate {
                        question,
                        origin_history_segment_index: None,
                    });
                }
            }
        }
    }

    let events = parse_events(text);
    history_segments.sort_by_key(|c| c.start_message);

    Ok(ParsedHistorySegmentOutput {
        history_segments,
        facts,
        facts_block_present,
        fact_syntax_failure,
        events,
        unprocessed_from,
        user_observations,
        primer_candidates,
    })
}

/// Validation applies discard-last boundary healing and returns only data safe to persist.
pub fn validate_history_summarizer_output(
    text: &str,
    chunk: &HistorySummarizerChunk,
    prior_history_segments: &[StoredHistorySegmentRange],
    options: ValidateOptions,
) -> Result<ValidatedChunk, HistorySummarizerValidationError> {
    let present_ordinals = chunk_present_ordinals(chunk);
    if let Some(error) = validate_chunk_coverage(chunk) {
        return Err(validation_error(format!(
            "HistorySummarizer chunk coverage invalid: {error}"
        )));
    }

    if let Some(error) = validate_stored_history_segments(prior_history_segments) {
        return Err(validation_error(format!(
            "Existing history_segments are invalid: {error}"
        )));
    }

    if let Some(last) = prior_history_segments.last() {
        if chunk.start_index <= last.end_message {
            return Err(validation_error(format!(
                "HistorySummarizer chunk starts at raw message {} but existing history_segments end at {}; expected a strictly newer raw message",
                chunk.start_index, last.end_message
            )));
        }
        if let Some(expected_start) = next_present_after(&present_ordinals, last.end_message)
            && chunk.start_index != expected_start
        {
            return Err(validation_error(format!(
                "HistorySummarizer chunk starts at raw message {} but existing history_segments end at {}; expected next present raw message {}",
                chunk.start_index, last.end_message, expected_start
            )));
        }
    }

    let mut parsed = parse_history_segment_output(text)?;
    if parsed.history_segments.is_empty() {
        return Err(validation_error(
            "HistorySummarizer returned no usable history_segments.",
        ));
    }

    heal_history_segment_gaps(
        &mut parsed.history_segments,
        &chunk.tool_only_ranges,
        &present_ordinals,
    );
    heal_terminal_completed_tool_arc(
        &mut parsed.history_segments,
        &mut parsed.unprocessed_from,
        &chunk.completed_tool_arcs,
        &present_ordinals,
        chunk.end_index,
    );

    let emitted = map_parsed_history_segments_to_chunk(
        &parsed.history_segments,
        chunk,
        options.sequence_offset,
    )
    .map_err(|error| {
        validation_error(format!(
            "HistorySummarizer returned invalid history_segment output: {error}"
        ))
    })?;

    if let Some(error) = validate_parsed_history_segments(
        &parsed.history_segments,
        chunk.start_index,
        chunk.end_index,
        &present_ordinals,
        parsed.unprocessed_from,
    ) {
        return Err(validation_error(format!(
            "HistorySummarizer returned invalid history_segment output: {error}"
        )));
    }
    if parsed
        .history_segments
        .last()
        .is_some_and(|history_segment| {
            boundary_splits_completed_tool_arc(
                history_segment.end_message.saturating_add(1),
                &chunk.completed_tool_arcs,
            )
        })
    {
        return Err(validation_error(
            "HistorySummarizer terminal boundary splits a completed tool invocation/result arc",
        ));
    }

    let mut history_segments = emitted;
    let emitted_count = history_segments.len();
    let mut discarded_last = false;
    if !options.in_emergency
        && !options.force_keep_last_history_segment
        && history_segments.len() >= 2
    {
        let last_end = history_segments
            .last()
            .map(|c| c.end_message)
            .unwrap_or(chunk.end_index);
        // TypeScript counts numeric ordinal distance, including retired message numbers, so sparse coordinate gaps still count.
        let lookahead_distance = chunk.end_index.saturating_sub(last_end);
        let previous_end = history_segments
            .get(history_segments.len().saturating_sub(2))
            .map(|history_segment| history_segment.end_message);
        let pop_would_split_arc = previous_end.is_some_and(|end| {
            boundary_splits_completed_tool_arc(end.saturating_add(1), &chunk.completed_tool_arcs)
        });
        if lookahead_distance <= BOUNDARY_HEALING_SLACK && !pop_would_split_arc {
            history_segments.pop();
            discarded_last = true;
        }
    }

    let offset = prior_history_segments
        .last()
        .map(|c| c.end_message.saturating_add(1))
        .unwrap_or(chunk.start_index);
    let last_new_end = history_segments.last().map(|c| c.end_message).unwrap_or(0);
    if last_new_end < offset {
        return Err(validation_error(format!(
            "no forward progress beyond raw message {}",
            offset.saturating_sub(1)
        )));
    }

    let persisted_count = history_segments.len() as u64;
    // Q30: history is valid from here on. The proposed fact set is judged as a whole: one bad citation rejects every fact, and the failure travels with the valid history instead of failing publication. A fact proves its source through its citations' native ordinals, not through a segment anchor, so a citation into a discarded provisional segment is a recorded rejection rather than a silent drop; every persisted segment, including a force-kept final one, is citable because the citation names exact native bytes rather than a boundary.
    let proposed = parsed.facts;
    let citable = history_segments
        .first()
        .zip(history_segments.last())
        .map(|(first, last)| first.start_message..=last.end_message);
    let (extraction, facts) = if !options.memory_enabled {
        (ExtractionOutcome::NotRequested, Vec::new())
    } else if let Some(failure) = parsed.fact_syntax_failure {
        (ExtractionOutcome::Rejected { failure }, Vec::new())
    } else if proposed.is_empty() {
        if parsed.facts_block_present {
            (ExtractionOutcome::NoFacts, Vec::new())
        } else {
            (ExtractionOutcome::NotRequested, Vec::new())
        }
    } else {
        match check_fact_set(&proposed, &chunk.aliases, citable) {
            Ok(()) => (
                ExtractionOutcome::Accepted {
                    count: proposed.len(),
                },
                proposed,
            ),
            Err(failure) => (ExtractionOutcome::Rejected { failure }, Vec::new()),
        }
    };
    // Discarding the last history_segment requires validation to skip unanchored producer output because its source history_segment cannot be proven.
    let events = parsed
        .events
        .into_iter()
        .filter(|event| {
            if options.force_keep_last_history_segment {
                matches!(event.at_history_segment, Some(index) if (1..persisted_count).contains(&index))
            } else {
                keep_side_channel(event.at_history_segment, persisted_count, discarded_last)
            }
        })
        .collect();
    let primer_candidates = parsed
        .primer_candidates
        .into_iter()
        .filter(|candidate| {
            !options.force_keep_last_history_segment
                && keep_side_channel(
                    candidate.origin_history_segment_index,
                    persisted_count,
                    discarded_last,
                )
        })
        .take(1)
        .collect();
    let user_observations = parsed
        .user_observations
        .into_iter()
        .filter(|observation| {
            !options.force_keep_last_history_segment
                && keep_side_channel(
                    observation.origin_history_segment_index,
                    persisted_count,
                    discarded_last,
                )
        })
        .collect();

    debug_assert!(history_segments.len() <= emitted_count);

    Ok(ValidatedChunk {
        history_segments,
        facts,
        extraction,
        events,
        primer_candidates,
        user_observations,
        // `unprocessed_from` is a publication floor: retired ordinals may be absent, so downstream scans advance to the next present input message.
        unprocessed_from: last_new_end.saturating_add(1),
        discarded_last,
    })
}

/// The validator checks already-persisted ranges before appending new output.
///
/// The store-pure check anchors at the first stored history_segment because only the live-aware fold can compare that start with the session's true first message.
pub fn validate_stored_history_segments(
    history_segments: &[StoredHistorySegmentRange],
) -> Option<String> {
    let first = history_segments.first()?;
    if first.end_message < first.start_message {
        return Some(format!(
            "invalid range {}-{}",
            first.start_message, first.end_message
        ));
    }

    let mut previous_end = first.end_message;
    for history_segment in &history_segments[1..] {
        if history_segment.end_message < history_segment.start_message {
            return Some(format!(
                "invalid range {}-{}",
                history_segment.start_message, history_segment.end_message
            ));
        }
        if history_segment.start_message <= previous_end {
            return Some(format!(
                "overlap before message {} (saw {}-{})",
                previous_end.saturating_add(1),
                history_segment.start_message,
                history_segment.end_message
            ));
        }
        previous_end = history_segment.end_message;
    }

    None
}

fn chunk_present_ordinals(chunk: &HistorySummarizerChunk) -> Vec<u64> {
    if !chunk.present_ordinals.is_empty() {
        return chunk.present_ordinals.clone();
    }
    chunk.lines.iter().map(|line| line.ordinal).collect()
}

fn validate_strictly_increasing_ordinals(ordinals: &[u64], label: &str) -> Option<String> {
    for pair in ordinals.windows(2) {
        let previous = pair[0];
        let current = pair[1];
        if current == previous {
            return Some(format!(
                "{label} contain duplicate raw message ordinal {current}"
            ));
        }
        if current < previous {
            return Some(format!(
                "{label} decrease from raw message {previous} to {current}"
            ));
        }
    }
    None
}

fn next_present_after(ordinals: &[u64], after: u64) -> Option<u64> {
    ordinals.iter().copied().find(|ordinal| *ordinal > after)
}

/// The chunk's ordinal lines must cover exactly the present input ordinals in the advertised raw range; missing retired ordinals are valid when absent from the real input set.
pub fn validate_chunk_coverage(chunk: &HistorySummarizerChunk) -> Option<String> {
    if chunk.present_ordinals.is_empty() {
        return validate_dense_chunk_coverage(chunk);
    }
    validate_chunk_coverage_against(chunk, &chunk.present_ordinals)
}

fn validate_dense_chunk_coverage(chunk: &HistorySummarizerChunk) -> Option<String> {
    let line_ordinals: Vec<u64> = chunk.lines.iter().map(|line| line.ordinal).collect();
    if let Some(error) = validate_strictly_increasing_ordinals(&line_ordinals, "chunk lines") {
        return Some(error);
    }
    if chunk.lines.is_empty() {
        return None;
    }

    let mut expected_ordinal = chunk.start_index;
    for line in &chunk.lines {
        if line.ordinal != expected_ordinal {
            return Some(format!(
                "chunk omits raw message {expected_ordinal} while still claiming coverage through {}",
                chunk.end_index
            ));
        }
        expected_ordinal = expected_ordinal.saturating_add(1);
    }

    if expected_ordinal.saturating_sub(1) != chunk.end_index {
        return Some(format!(
            "chunk omits raw message {} while still claiming coverage through {}",
            expected_ordinal, chunk.end_index
        ));
    }

    None
}

fn validate_chunk_coverage_against(
    chunk: &HistorySummarizerChunk,
    present_ordinals: &[u64],
) -> Option<String> {
    if let Some(error) = validate_strictly_increasing_ordinals(present_ordinals, "input ordinals") {
        return Some(error);
    }

    let line_ordinals: Vec<u64> = chunk.lines.iter().map(|line| line.ordinal).collect();
    if let Some(error) = validate_strictly_increasing_ordinals(&line_ordinals, "chunk lines") {
        return Some(error);
    }

    if let Some(outside) = line_ordinals
        .iter()
        .find(|ordinal| **ordinal < chunk.start_index || **ordinal > chunk.end_index)
    {
        return Some(format!(
            "chunk line raw message {outside} is outside claimed coverage {}-{}",
            chunk.start_index, chunk.end_index
        ));
    }

    let expected: Vec<u64> = present_ordinals
        .iter()
        .copied()
        .filter(|ordinal| *ordinal >= chunk.start_index && *ordinal <= chunk.end_index)
        .collect();

    for (line, expected) in line_ordinals.iter().zip(expected.iter()) {
        if line == expected {
            continue;
        }
        if line > expected {
            return Some(format!(
                "chunk omits raw message {expected} while still claiming coverage through {}",
                chunk.end_index
            ));
        }
        return Some(format!(
            "chunk includes raw message {line} that is not present in input range {}-{}",
            chunk.start_index, chunk.end_index
        ));
    }

    if let Some(missing) = expected.get(line_ordinals.len()) {
        return Some(format!(
            "chunk omits raw message {missing} while still claiming coverage through {}",
            chunk.end_index
        ));
    }

    if let Some(extra) = line_ordinals.get(expected.len()) {
        return Some(format!(
            "chunk includes raw message {extra} that is not present in input range {}-{}",
            chunk.start_index, chunk.end_index
        ));
    }

    None
}

fn parse_events(text: &str) -> Vec<ParsedEvent> {
    let Some(block_caps) = events_block_regex().captures(text) else {
        return Vec::new();
    };
    let block = block_caps.get(1).map(|m| m.as_str()).unwrap_or_default();
    let mut events = Vec::new();

    // Rust regexes lack backreferences, so the parser matches event opening tags and searches for the corresponding literal close tag.
    // Event child fields lack `at_history_segment`, so the parser cannot mistake them for event opens.
    for event_caps in event_open_regex().captures_iter(block) {
        let Some(full_match) = event_caps.get(0) else {
            continue;
        };
        let kind = event_caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        let close_tag = format!("</{kind}>");
        let body_start = full_match.end();
        let Some(relative_body_end) = block[body_start..].find(&close_tag) else {
            continue;
        };
        let body = &block[body_start..body_start + relative_body_end];

        let mut fields = BTreeMap::new();
        for field_caps in event_field_regex().captures_iter(body) {
            let name = field_caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let closing = field_caps.get(3).map(|m| m.as_str()).unwrap_or_default();
            if name != closing {
                continue;
            }
            let value = field_caps
                .get(2)
                .map(|m| unescape_xml(m.as_str().trim()))
                .unwrap_or_default();
            if !value.is_empty() {
                fields.insert(name.to_string(), value);
            }
        }
        events.push(ParsedEvent {
            kind: kind.to_string(),
            at_history_segment: event_caps
                .get(2)
                .and_then(|m| m.as_str().parse::<u64>().ok()),
            fields,
        });
    }
    events
}

fn boundary_splits_completed_tool_arc(boundary: u64, arcs: &[MessageRange]) -> bool {
    arcs.iter()
        .any(|arc| completed_tool_arc_crosses_boundary(arc.start, arc.end, boundary))
}

fn heal_terminal_completed_tool_arc(
    history_segments: &mut [ParsedHistorySegment],
    unprocessed_from: &mut Option<u64>,
    arcs: &[MessageRange],
    present_ordinals: &[u64],
    chunk_end: u64,
) {
    let Some(last) = history_segments.last_mut() else {
        return;
    };
    let original_end = last.end_message;
    for _ in 0..=arcs.len() {
        let boundary = last.end_message.saturating_add(1);
        let next_end = arcs
            .iter()
            .filter(|arc| {
                arc.end <= chunk_end
                    && completed_tool_arc_crosses_boundary(arc.start, arc.end, boundary)
            })
            .map(|arc| arc.end)
            .max()
            .unwrap_or(last.end_message);
        if next_end == last.end_message {
            break;
        }
        last.end_message = next_end;
    }
    if last.end_message != original_end
        && let Some(unprocessed) = unprocessed_from.as_mut()
    {
        *unprocessed = next_present_after(present_ordinals, last.end_message)
            .unwrap_or_else(|| last.end_message.saturating_add(1));
    }
}

fn heal_history_segment_gaps(
    history_segments: &mut [ParsedHistorySegment],
    tool_only_ranges: &[MessageRange],
    present_ordinals: &[u64],
) {
    for i in 1..history_segments.len() {
        let gap_start = history_segments[i - 1].end_message.saturating_add(1);
        let gap_end = history_segments[i].start_message.saturating_sub(1);
        if gap_end < gap_start {
            continue;
        }
        let omitted_present: Vec<u64> = present_ordinals
            .iter()
            .copied()
            .filter(|ordinal| *ordinal >= gap_start && *ordinal <= gap_end)
            .collect();
        if omitted_present.is_empty() {
            continue;
        }
        let fully_inside_tool_only = omitted_present.iter().all(|ordinal| {
            tool_only_ranges
                .iter()
                .any(|range| range.start <= *ordinal && range.end >= *ordinal)
        });
        // Only tool-only gaps may be absorbed; narrative gaps reject before the publish path advances the durable boundary.
        if fully_inside_tool_only {
            history_segments[i - 1].end_message = *omitted_present
                .last()
                .expect("non-empty omitted present ordinals checked above");
        }
    }
}

fn map_parsed_history_segments_to_chunk(
    history_segments: &[ParsedHistorySegment],
    chunk: &HistorySummarizerChunk,
    sequence_offset: u64,
) -> Result<Vec<ValidatedHistorySegment>, String> {
    let mut mapped = Vec::with_capacity(history_segments.len());
    for (index, history_segment) in history_segments.iter().enumerate() {
        let start_line = chunk
            .lines
            .iter()
            .find(|line| line.ordinal == history_segment.start_message);
        let end_line = chunk
            .lines
            .iter()
            .find(|line| line.ordinal == history_segment.end_message);
        let (Some(start_line), Some(end_line)) = (start_line, end_line) else {
            return Err(format!(
                "HistorySegment range {}-{} does not map to raw session lines {}-{}",
                history_segment.start_message,
                history_segment.end_message,
                chunk.start_index,
                chunk.end_index
            ));
        };
        if !end_line.anchorable || end_line.message_id.is_empty() {
            return Err(format!(
                "HistorySegment ending at raw message {} cannot anchor a boundary because that message has no flat blocks",
                history_segment.end_message
            ));
        }
        mapped.push(ValidatedHistorySegment {
            sequence: sequence_offset + index as u64,
            start_message: history_segment.start_message,
            end_message: history_segment.end_message,
            start_message_id: start_line.message_id.clone(),
            end_message_id: end_line.message_id.clone(),
            title: history_segment.title.clone(),
            content: history_segment.content.clone(),
            p1: history_segment.p1.clone(),
            p2: history_segment.p2.clone(),
            p3: history_segment.p3.clone(),
            p4: history_segment.p4.clone(),
            importance: history_segment.importance,
            episode_type: history_segment.episode_type.clone(),
        });
    }
    Ok(mapped)
}

fn validate_parsed_history_segments(
    history_segments: &[ParsedHistorySegment],
    chunk_start: u64,
    chunk_end: u64,
    present_ordinals: &[u64],
    unprocessed_from: Option<u64>,
) -> Option<String> {
    let chunk_ordinals: Vec<u64> = present_ordinals
        .iter()
        .copied()
        .filter(|ordinal| *ordinal >= chunk_start && *ordinal <= chunk_end)
        .collect();
    let mut expected_start = chunk_ordinals.first().copied();

    for (index, history_segment) in history_segments.iter().enumerate() {
        // P1 is required at the v2 boundary; absent P2-P4 use the parser's denser-tier fallbacks, but the flat v1 shape retries.
        match history_segment.p1.as_deref() {
            Some(p1) if !p1.trim().is_empty() => {}
            _ => {
                return Some(format!(
                    "history_segment {} is missing the tiered paraphrase structure (p1..p4); re-emit with all four tiers",
                    index + 1
                ));
            }
        }
        if history_segment.end_message < history_segment.start_message {
            return Some(format!(
                "invalid range {}-{}",
                history_segment.start_message, history_segment.end_message
            ));
        }
        if history_segment.start_message < chunk_start || history_segment.end_message > chunk_end {
            return Some(format!(
                "range {}-{} is outside chunk {}-{}",
                history_segment.start_message, history_segment.end_message, chunk_start, chunk_end
            ));
        }
        if !chunk_ordinals.contains(&history_segment.start_message) {
            return Some(format!(
                "range start {} is not a present raw message in chunk {}-{}",
                history_segment.start_message, chunk_start, chunk_end
            ));
        }
        if !chunk_ordinals.contains(&history_segment.end_message) {
            return Some(format!(
                "range end {} is not a present raw message in chunk {}-{}",
                history_segment.end_message, chunk_start, chunk_end
            ));
        }
        let Some(expected) = expected_start else {
            return Some(format!(
                "range {}-{} starts after chunk coverage already ended",
                history_segment.start_message, history_segment.end_message
            ));
        };
        if history_segment.start_message != expected {
            if history_segment.start_message < expected {
                return Some(format!(
                    "overlap before message {expected} (saw {}-{})",
                    history_segment.start_message, history_segment.end_message
                ));
            }
            return Some(format!(
                "gap before present message {} (expected {expected})",
                history_segment.start_message
            ));
        }
        expected_start = next_present_after(&chunk_ordinals, history_segment.end_message);
    }

    if let Some(unprocessed_from) = unprocessed_from {
        if let Some(expected) = expected_start {
            if unprocessed_from != expected {
                return Some(format!(
                    "<unprocessed_from> {unprocessed_from} does not match next uncovered message {expected}"
                ));
            }
            return None;
        }
        if unprocessed_from == chunk_end.saturating_add(1) {
            return None;
        }
        if unprocessed_from < chunk_start || unprocessed_from > chunk_end {
            return Some(format!(
                "<unprocessed_from> {unprocessed_from} is outside chunk {chunk_start}-{chunk_end}"
            ));
        }
        return Some(format!(
            "<unprocessed_from> {unprocessed_from} does not match completed chunk boundary {}",
            chunk_end.saturating_add(1)
        ));
    }

    if let Some(expected) = expected_start {
        return Some(format!(
            "output left uncovered messages {expected}-{chunk_end} without <unprocessed_from>"
        ));
    }

    None
}

fn keep_side_channel(
    origin_history_segment_index: Option<u64>,
    persisted_count: u64,
    discarded_last: bool,
) -> bool {
    if discarded_last {
        return false;
    }
    match origin_history_segment_index {
        Some(index) => (1..=persisted_count).contains(&index),
        None => !discarded_last,
    }
}

fn capture_string(regex: &Regex, haystack: &str) -> Option<String> {
    regex
        .captures(haystack)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
}

fn capture_u64(regex: &Regex, haystack: &str) -> Option<u64> {
    regex
        .captures(haystack)
        .and_then(|caps| caps.get(1).and_then(|m| m.as_str().parse::<u64>().ok()))
}

fn extract_tier(inner: &str, index: usize) -> Option<String> {
    let open_match = tier_open_regexes()[index].captures(inner)?;
    let full = open_match.get(0)?;
    // A self-closing `<p4/>` or `<p4 />` represents an empty tier.
    if open_match.get(1).map(|m| m.as_str()) == Some("/") {
        return Some(String::new());
    }
    let rest = &inner[full.end()..];
    // The parser bounds a tier body at the next closing tier tag; without a close tag, it stops at the history_segment end or an intervening opener.
    let end = tier_close_any_regex()
        .find(rest)
        .map(|m| m.start())
        .unwrap_or(rest.len());
    let mut body = &rest[..end];
    // The over-capture guard cuts tier content at any subsequent tier opener before the closing tag.
    if let Some(open_inside) = tier_open_any_regex().find(body) {
        body = &body[..open_inside.start()];
    }
    Some(unescape_xml(body.trim()))
}

fn split_anchor_prefix(text: &str) -> (Option<u64>, String) {
    if let Some(caps) = side_channel_anchor_regex().captures(text) {
        let anchor = caps.get(1).and_then(|m| m.as_str().parse::<u64>().ok());
        let content = caps
            .get(2)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();
        return (anchor, content);
    }
    (None, text.trim().to_string())
}

/// Decodes the five XML entities in one pass, so `&amp;lt;` yields the literal
/// text `&lt;` rather than `<`.
fn unescape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let entity = &rest[start..];
        let decoded = [
            ("&amp;", '&'),
            ("&apos;", '\''),
            ("&quot;", '"'),
            ("&lt;", '<'),
            ("&gt;", '>'),
        ]
        .into_iter()
        .find_map(|(name, ch)| entity.strip_prefix(name).map(|after| (ch, after)));
        match decoded {
            Some((ch, after)) => {
                out.push(ch);
                rest = after;
            }
            None => {
                out.push('&');
                rest = &entity[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn output_document_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Unanchored and lazy: the first complete root wins, and the caller rejects any other
    // `output` tag before or after it, so a fenced or prose-wrapped document parses while two
    // documents still fail.
    RE.get_or_init(|| Regex::new(r"(?is)<output(?:\s[^>]*)?>(?P<body>.*?)</output\s*>").unwrap())
}

fn output_tag_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)</?output(?:\s[^>]*)?>").unwrap())
}

fn history_segment_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?s)<history_segment\s+([^>]*?)\s*>(.*?)</history_segment>"#).unwrap()
    })
}

fn attr_start_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"\bstart="(\d+)""#).unwrap())
}

fn attr_end_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"\bend="(\d+)""#).unwrap())
}

fn attr_title_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"\btitle="([^"]*)""#).unwrap())
}

fn attr_episode_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"\bepisode_type="([^"]*)""#).unwrap())
}

fn attr_importance_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"\bimportance="(\d+)""#).unwrap())
}

/// Self-closing tier tags represent empty tiers.
/// Mismatched closing digits terminate an opened tier.
fn tier_open_regexes() -> &'static [Regex; 4] {
    static RE: OnceLock<[Regex; 4]> = OnceLock::new();
    RE.get_or_init(|| {
        [
            Regex::new(r"<p1\s*(/?)>").unwrap(),
            Regex::new(r"<p2\s*(/?)>").unwrap(),
            Regex::new(r"<p3\s*(/?)>").unwrap(),
            Regex::new(r"<p4\s*(/?)>").unwrap(),
        ]
    })
}

/// Any closing tag matching `</p\d` bounds an opened tier's body.
/// Any closing tier tag terminates an opened tier even when its digit differs from the opener's.
fn tier_close_any_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"</p\d").unwrap())
}

/// Any later tier opener terminates the current tier body.
fn tier_open_any_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<p\d").unwrap())
}

fn facts_block_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?s)<facts>(.*?)</facts>"#).unwrap())
}

fn events_block_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?s)<events>(.*?)</events>"#).unwrap())
}

fn category_block_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?s)<(PROJECT_RULES|ARCHITECTURE|CONSTRAINTS|CONFIG_VALUES|NAMING)>(.*?)</(PROJECT_RULES|ARCHITECTURE|CONSTRAINTS|CONFIG_VALUES|NAMING)>"#,
        )
        .unwrap()
    })
}

fn fact_item_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?m)^[ \t]*\*[ \t]*(.*)$"#).unwrap())
}

fn unprocessed_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"<unprocessed_from>(\d+)</unprocessed_from>"#).unwrap())
}

fn user_observations_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?s)<user_observations>(.*?)</user_observations>"#).unwrap())
}

fn user_obs_item_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?m)^\s*\*\s*(.+)$"#).unwrap())
}

fn primer_candidates_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?s)<primer_candidates>(.*?)</primer_candidates>"#).unwrap())
}

fn primer_element_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?s)<primer\s+at_history_segment="(\d+)"\s*>(.*?)</primer>"#).unwrap()
    })
}

fn primer_item_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?m)^\s*(?:\*|-|\d+\.)\s*(.+)$"#).unwrap())
}

fn event_open_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"<([a-z_]+)\s+at_history_segment="(\d+)"\s*>"#).unwrap())
}

fn event_field_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?s)<([a-z_]+)\s*>(.*?)</([a-z_]+)>"#).unwrap())
}

fn side_channel_anchor_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"^\s*[\[(]\s*(?:at_history_segment|origin_history_segment)\s*=\s*"?(\d+)"?\s*[\])]\s*(.+)$"#,
        )
        .unwrap()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_summarizer_citations::FrozenAlias;

    /// A producer that escapes `&lt;` writes `&amp;lt;`; decoding in one pass
    /// yields the literal `&lt;` instead of decoding the exposed entity again.
    #[test]
    fn unescape_xml_decodes_each_entity_once() {
        assert_eq!(unescape_xml("&amp;lt;"), "&lt;");
        assert_eq!(unescape_xml("&amp;amp;"), "&amp;");
        assert_eq!(unescape_xml("&amp;quot;x&amp;apos;"), "&quot;x&apos;");
        assert_eq!(
            unescape_xml("a &lt;b&gt; &quot;c&quot; &apos;d&apos; &amp; e"),
            "a <b> \"c\" 'd' & e"
        );
        assert_eq!(unescape_xml("&unknown; & &lt"), "&unknown; & &lt");
        assert_eq!(unescape_xml("plain"), "plain");
    }

    #[derive(Debug, Deserialize)]
    struct GoldenInput {
        text: String,
        chunk: HistorySummarizerChunk,
        #[serde(default)]
        prior_history_segments: Vec<StoredHistorySegmentRange>,
        #[serde(default)]
        options: ValidateOptions,
    }

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct GoldenVerdict {
        ok: bool,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        result: Option<ValidatedChunk>,
    }

    #[derive(Debug, Deserialize)]
    struct GoldenCase {
        label: String,
        input: GoldenInput,
        parsed: ParsedHistorySegmentOutput,
        validation: GoldenVerdict,
    }

    fn verdict(result: Result<ValidatedChunk, HistorySummarizerValidationError>) -> GoldenVerdict {
        match result {
            Ok(result) => GoldenVerdict {
                ok: true,
                error: None,
                result: Some(result),
            },
            Err(error) => GoldenVerdict {
                ok: false,
                error: Some(error.message),
                result: None,
            },
        }
    }

    fn chunk(start: u64, end: u64) -> HistorySummarizerChunk {
        HistorySummarizerChunk {
            aliases: Default::default(),
            start_index: start,
            end_index: end,
            lines: (start..=end)
                .map(|ordinal| ChunkLine {
                    ordinal,
                    message_id: format!("msg-{ordinal}"),
                    anchorable: true,
                })
                .collect(),
            present_ordinals: (start..=end).collect(),
            tool_only_ranges: Vec::new(),
            completed_tool_arcs: Vec::new(),
        }
    }

    /// `chunk` with one frozen alias per line whose presented text is `hello world`.
    fn aliased_chunk(start: u64, end: u64) -> HistorySummarizerChunk {
        let mut chunk = chunk(start, end);
        for ordinal in start..=end {
            chunk.aliases.issue(FrozenAlias {
                message_id: format!("msg-{ordinal}"),
                ordinal,
                block_ids: vec![format!("msg-{ordinal}#0")],
                block_hashes: vec!["0".repeat(64)],
                presented: "hello world".into(),
                ..FrozenAlias::default()
            });
        }
        chunk
    }

    fn xml(history_segments: &[(u64, u64, &str)], unprocessed_from: u64, extra: &str) -> String {
        let body = history_segments
            .iter()
            .map(|(start, end, title)| {
                format!(
                    r#"<history_segment start="{start}" end="{end}" title="{title}" episode_type="feature" importance="50"><p1>{title} full</p1><p2>{title} short</p2><p3>{title}</p3><p4 /></history_segment>"#
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "<output><history_segments>{body}</history_segments>{extra}<meta><unprocessed_from>{unprocessed_from}</unprocessed_from></meta></output>"
        )
    }

    #[test]
    fn validate_golden_matches_typescript_oracle() {
        let raw = include_str!("../testdata/validate-golden.json");
        let cases: Vec<GoldenCase> = serde_json::from_str(raw).expect("parse validate golden");
        assert!(!cases.is_empty(), "empty validate golden");

        for case in &cases {
            match parse_history_segment_output(&case.input.text) {
                Ok(parsed) => assert_eq!(parsed, case.parsed, "parsed mismatch in {}", case.label),
                Err(error) => {
                    assert!(
                        !case.validation.ok && error.message.contains("<output> root document"),
                        "only malformed envelopes may diverge from the permissive TypeScript parser in {}",
                        case.label
                    );
                    continue;
                }
            }

            let got = verdict(validate_history_summarizer_output(
                &case.input.text,
                &case.input.chunk,
                &case.input.prior_history_segments,
                case.input.options,
            ));
            assert_eq!(
                got, case.validation,
                "validation mismatch in {}",
                case.label
            );
        }
    }

    #[test]
    fn tierless_history_segments_reject_while_p1_only_output_keeps_soft_fallbacks() {
        let flat = r#"<output><history_segment start="1" end="2" title="flat">flat summary</history_segment><meta><unprocessed_from>3</unprocessed_from></meta></output>"#;
        let error =
            validate_history_summarizer_output(flat, &chunk(1, 2), &[], ValidateOptions::default())
                .expect_err("flat v1 output must re-enter the producer retry chain");
        assert!(error.message.contains(
            "history_segment 1 is missing the tiered paraphrase structure (p1..p4); re-emit with all four tiers"
        ));

        let p1_only = r#"<output><history_segment start="1" end="2" title="partial"><p1>full summary</p1></history_segment><meta><unprocessed_from>3</unprocessed_from></meta></output>"#;
        let validated = validate_history_summarizer_output(
            p1_only,
            &chunk(1, 2),
            &[],
            ValidateOptions::default(),
        )
        .expect("P1 is enough for the parser's deliberate soft-tier fallback");
        let history_segment = &validated.history_segments[0];
        assert_eq!(history_segment.p1.as_deref(), Some("full summary"));
        assert_eq!(history_segment.p2.as_deref(), Some("full summary"));
        assert_eq!(history_segment.p3.as_deref(), Some("full summary"));
        assert_eq!(history_segment.p4.as_deref(), Some(""));
    }

    #[test]
    fn mismatched_tier_close_parses_leniently_while_tierless_output_still_rejects() {
        // Exact observed provider shape: deepseek-v4-flash-free closes
        // <p1> with </p2>. The lenient parser terminates the opened <p1> at the
        // NEXT closing tier tag (any digit) and the real <p2> still parses, so
        // validation passes (the legacy=0 tiered path) instead of retrying.
        let mangled = r#"<output><history_segment start="1" end="2" title="mangled" importance="55"><p1>
full narrative
</p2>
<p2>condensed</p2><p3>outcome</p3><p4/></history_segment><meta><unprocessed_from>3</unprocessed_from></meta></output>"#;
        let validated = validate_history_summarizer_output(
            mangled,
            &chunk(1, 2),
            &[],
            ValidateOptions::default(),
        )
        .expect("mismatched close must parse leniently into a tiered history_segment");
        let history_segment = &validated.history_segments[0];
        assert_eq!(history_segment.p1.as_deref(), Some("full narrative"));
        assert_eq!(history_segment.content, "full narrative"); // mirrors P1
        assert_eq!(history_segment.p2.as_deref(), Some("condensed"));
        assert_eq!(history_segment.p3.as_deref(), Some("outcome"));
        assert_eq!(history_segment.p4.as_deref(), Some(""));

        let flat = r#"<output><history_segment start="1" end="2" title="flat">flat summary</history_segment><meta><unprocessed_from>3</unprocessed_from></meta></output>"#;
        let error =
            validate_history_summarizer_output(flat, &chunk(1, 2), &[], ValidateOptions::default())
                .expect_err("tier-free flat output must still reject");
        assert!(
            error
                .message
                .contains("missing the tiered paraphrase structure (p1..p4)")
        );
    }

    #[test]
    fn lenient_tier_extraction_bounds_bodies_and_guards_overcapture() {
        let missing_close = r#"<output><history_segments><history_segment start="1" end="2" title="x" importance="50"><p1>first body<p2>second body</p2><p3>third</p3><p4/></history_segment></history_segments></output>"#;
        let parsed = parse_history_segment_output(missing_close).expect("parse missing-close");
        let history_segment = &parsed.history_segments[0];
        assert_eq!(history_segment.p1.as_deref(), Some("first body"));
        assert_eq!(history_segment.p2.as_deref(), Some("second body"));
        assert_eq!(history_segment.p3.as_deref(), Some("third"));

        // A later tier opener terminates the current tier body.
        let overcapture = r#"<output><history_segments><history_segment start="1" end="2" title="x" importance="50"><p1>alpha<p2>beta</p1><p3>gamma</p3><p4/></history_segment></history_segments></output>"#;
        let parsed = parse_history_segment_output(overcapture).expect("parse over-capture");
        let history_segment = &parsed.history_segments[0];
        assert_eq!(history_segment.p1.as_deref(), Some("alpha"));
        assert_eq!(history_segment.p2.as_deref(), Some("beta"));
    }

    #[test]
    fn stored_history_segment_validation_is_basis_agnostic_and_allows_sparse_gaps() {
        assert_eq!(
            validate_stored_history_segments(&[
                StoredHistorySegmentRange {
                    start_message: 0,
                    end_message: 4,
                },
                StoredHistorySegmentRange {
                    start_message: 5,
                    end_message: 8,
                },
            ]),
            None
        );

        assert_eq!(
            validate_stored_history_segments(&[
                StoredHistorySegmentRange {
                    start_message: 5,
                    end_message: 7,
                },
                StoredHistorySegmentRange {
                    start_message: 9,
                    end_message: 10,
                },
            ]),
            None,
            "store-pure validation cannot distinguish retired ordinals from gaps",
        );

        let overlap = validate_stored_history_segments(&[
            StoredHistorySegmentRange {
                start_message: 0,
                end_message: 4,
            },
            StoredHistorySegmentRange {
                start_message: 4,
                end_message: 6,
            },
        ])
        .expect("overlap rejected");
        assert!(overlap.contains("overlap before message 5"));
    }

    #[test]
    fn chunk_coverage_rejects_duplicate_and_decreasing_ordinals() {
        let duplicate = HistorySummarizerChunk {
            aliases: Default::default(),
            start_index: 1,
            end_index: 2,
            lines: vec![
                ChunkLine {
                    ordinal: 1,
                    message_id: "m1#0".into(),
                    anchorable: true,
                },
                ChunkLine {
                    ordinal: 1,
                    message_id: "m1-dup#0".into(),
                    anchorable: true,
                },
                ChunkLine {
                    ordinal: 2,
                    message_id: "m2#0".into(),
                    anchorable: true,
                },
            ],
            present_ordinals: vec![1, 1, 2],
            tool_only_ranges: Vec::new(),
            completed_tool_arcs: Vec::new(),
        };
        let duplicate_error = validate_chunk_coverage(&duplicate).expect("duplicate rejected");
        assert!(duplicate_error.contains("duplicate raw message ordinal 1"));

        let decreasing = HistorySummarizerChunk {
            aliases: Default::default(),
            start_index: 1,
            end_index: 3,
            lines: vec![
                ChunkLine {
                    ordinal: 1,
                    message_id: "m1#0".into(),
                    anchorable: true,
                },
                ChunkLine {
                    ordinal: 3,
                    message_id: "m3#0".into(),
                    anchorable: true,
                },
                ChunkLine {
                    ordinal: 2,
                    message_id: "m2#0".into(),
                    anchorable: true,
                },
            ],
            present_ordinals: vec![1, 2, 3],
            tool_only_ranges: Vec::new(),
            completed_tool_arcs: Vec::new(),
        };
        let decreasing_error = validate_chunk_coverage(&decreasing).expect("decrease rejected");
        assert!(decreasing_error.contains("chunk lines decrease from raw message 3 to 2"));
    }

    #[test]
    fn parser_requires_one_complete_output_root() {
        let rootless = r#"<history_segments><history_segment start="1" end="1" title="t"><p1>x</p1></history_segment></history_segments><meta><unprocessed_from>2</unprocessed_from></meta>"#;
        let truncated = r#"<output><history_segments><history_segment start="1" end="1" title="t"><p1>x</p1></history_segment></history_segments>"#;
        let nested = format!("<output>{}</output>", xml(&[(1, 1, "nested")], 2, ""));

        for malformed in [rootless, truncated, nested.as_str()] {
            let error = parse_history_segment_output(malformed).expect_err("invalid root rejected");
            assert!(error.message.contains("<output> root document"));
        }
        // Two complete documents are still two, wherever the second one sits.
        let doubled = format!(
            "{}\n{}",
            xml(&[(1, 1, "first")], 2, ""),
            xml(&[(1, 1, "second")], 2, "")
        );
        let error = parse_history_segment_output(&doubled).expect_err("two roots rejected");
        assert!(error.message.contains("exactly one <output>"));
        // A stray tag before the root counts too.
        let prefixed = format!("</output>\n{}", xml(&[(1, 1, "first")], 2, ""));
        let error = parse_history_segment_output(&prefixed).expect_err("stray prefix tag rejected");
        assert!(error.message.contains("exactly one <output>"));
    }

    /// A model that fences the document in Markdown or adds a sentence after it still
    /// delivered one complete root; the parser reads that root and ignores the wrapper.
    #[test]
    fn parser_ignores_a_markdown_fence_and_prose_around_the_single_root() {
        let document = xml(&[(1, 1, "fenced")], 2, "");
        let wrapped = format!(
            "Here is the summary:\n```xml\n{document}\n```\n\nACK. Message [1] processed; nothing else was extracted."
        );
        let parsed = parse_history_segment_output(&wrapped).expect("fenced root parses");
        assert_eq!(parsed.history_segments.len(), 1);
        assert_eq!(parsed.history_segments[0].title, "fenced");
        assert_eq!(parsed.unprocessed_from, Some(2));
        let bare = parse_history_segment_output(&document).expect("bare root parses");
        assert_eq!(parsed.history_segments, bare.history_segments);
    }

    #[test]
    fn history_segment_end_must_be_anchorable() {
        let mut chunk = chunk(1, 2);
        chunk.lines[1].message_id.clear();
        chunk.lines[1].anchorable = false;
        let text = xml(&[(1, 2, "zero-block tail")], 3, "");

        let error =
            validate_history_summarizer_output(&text, &chunk, &[], ValidateOptions::default())
                .expect_err("a zero-block endpoint cannot publish");
        assert!(error.message.contains("cannot anchor a boundary"));

        chunk.lines[1].message_id = "m2#0".to_string();
        chunk.lines[1].anchorable = true;
        let validated =
            validate_history_summarizer_output(&text, &chunk, &[], ValidateOptions::default())
                .expect("a real flat-block endpoint remains publishable");
        assert_eq!(validated.history_segments[0].end_message_id, "m2#0");
    }

    #[test]
    fn terminal_unprocessed_boundary_closes_a_completed_arc_forward() {
        let mut row_chunk = chunk(98, 128);
        row_chunk.completed_tool_arcs = vec![MessageRange {
            start: 123,
            end: 124,
        }];
        let text = xml(&[(98, 123, "covered prefix")], 124, "");

        let validated = validate_history_summarizer_output(
            &text,
            &row_chunk,
            &[],
            ValidateOptions {
                in_emergency: true,
                ..ValidateOptions::default()
            },
        )
        .expect("the terminal boundary should close the completed arc forward");

        assert_eq!(validated.history_segments[0].end_message, 124);
        assert_eq!(validated.history_segments[0].end_message_id, "msg-124");
        assert_eq!(validated.unprocessed_from, 125);
    }

    #[test]
    fn completed_arc_past_chunk_end_rejects_instead_of_publishing_half() {
        let mut short_chunk = chunk(1, 2);
        short_chunk.completed_tool_arcs = vec![MessageRange { start: 2, end: 3 }];
        let text = xml(&[(1, 2, "prefix")], 3, "");

        let error = validate_history_summarizer_output(
            &text,
            &short_chunk,
            &[],
            ValidateOptions {
                in_emergency: true,
                ..ValidateOptions::default()
            },
        )
        .expect_err("an unavailable result must keep the whole arc out of durable coverage");

        assert!(error.message.contains("terminal boundary splits"));
    }

    #[test]
    fn discard_last_cannot_reopen_a_completed_arc() {
        let mut row_chunk = chunk(98, 128);
        row_chunk.completed_tool_arcs = vec![MessageRange {
            start: 123,
            end: 124,
        }];
        let text = xml(&[(98, 123, "prefix"), (124, 128, "lookahead")], 129, "");

        let validated =
            validate_history_summarizer_output(&text, &row_chunk, &[], ValidateOptions::default())
                .expect("discard-last healing must preserve the whole completed arc");

        assert!(!validated.discarded_last);
        assert_eq!(validated.history_segments.len(), 2);
        assert_eq!(validated.history_segments.last().unwrap().end_message, 128);
    }

    #[test]
    fn discard_last_uses_numeric_sparse_ordinal_distance() {
        let sparse = HistorySummarizerChunk {
            aliases: Default::default(),
            start_index: 1,
            end_index: 100,
            lines: [1, 2, 100]
                .into_iter()
                .map(|ordinal| ChunkLine {
                    ordinal,
                    message_id: format!("m{ordinal}#0"),
                    anchorable: true,
                })
                .collect(),
            present_ordinals: vec![1, 2, 100],
            tool_only_ranges: Vec::new(),
            completed_tool_arcs: Vec::new(),
        };
        let text = xml(&[(1, 1, "first"), (2, 2, "provisional")], 100, "");

        let validated =
            validate_history_summarizer_output(&text, &sparse, &[], ValidateOptions::default())
                .expect("sparse chunk validates");
        assert!(!validated.discarded_last);
        assert_eq!(validated.history_segments.len(), 2);
        assert_eq!(validated.unprocessed_from, 3);
    }

    #[test]
    fn zero_side_channel_anchor_is_suppressed() {
        let extra = r#"
<facts><PROJECT_RULES>
* [at_history_segment=0] [s1:0-5] Drop the zero rule.
* [at_history_segment=1] [s1:0-5] Keep the first rule.
</PROJECT_RULES></facts>
<events>
<causal_incident at_history_segment="0"><summary>zero event</summary></causal_incident>
<trajectory_correction at_history_segment="1"><summary>first event</summary></trajectory_correction>
</events>
<user_observations>
* [at_history_segment=0] Drop the zero observation.
* [at_history_segment=1] Keep the first observation.
</user_observations>
<primer_candidates>
<primer at_history_segment="0">Drop the zero primer?</primer>
<primer at_history_segment="1">Keep the first primer?</primer>
</primer_candidates>
"#;
        let text = xml(&[(1, 2, "only")], 3, extra);
        let validated = validate_history_summarizer_output(
            &text,
            &aliased_chunk(1, 2),
            &[],
            ValidateOptions::default(),
        )
        .expect("valid history_segment publishes");

        // A fact's source is its citation's native ordinal, not the segment anchor: both facts cite `s1` inside the kept segment, so both are admitted even though one carries a zero anchor.
        assert_eq!(validated.facts.len(), 2);
        assert_eq!(validated.facts[1].content, "Keep the first rule.");
        assert_eq!(
            validated.extraction,
            ExtractionOutcome::Accepted { count: 2 }
        );
        assert_eq!(validated.events.len(), 1);
        assert_eq!(validated.events[0].kind, "trajectory_correction");
        assert_eq!(validated.user_observations.len(), 1);
        assert_eq!(
            validated.user_observations[0].content,
            "Keep the first observation."
        );
        assert_eq!(validated.primer_candidates.len(), 1);
        assert_eq!(
            validated.primer_candidates[0].question,
            "Keep the first primer?"
        );
    }

    #[test]
    fn discarded_last_suppresses_every_side_channel_for_the_whole_run() {
        let extra = r#"
<facts>
<PROJECT_RULES>
* [at_history_segment=1] [s1:0-5] Keep the earlier rule.
* [at_history_segment=2] [s3:0-5] Drop the provisional rule.
</PROJECT_RULES>
</facts>
<events>
<causal_incident at_history_segment="1"><summary>kept event</summary></causal_incident>
<trajectory_correction at_history_segment="2"><summary>dropped event</summary></trajectory_correction>
</events>
<user_observations>
* [at_history_segment=1] Keep the earlier observation.
* [at_history_segment=2] Drop the provisional observation.
</user_observations>
<primer_candidates>
<primer at_history_segment="1">How does the kept subsystem work?</primer>
<primer at_history_segment="2">How does the dropped subsystem work?</primer>
</primer_candidates>
"#;
        let text = xml(&[(1, 2, "first"), (3, 4, "second")], 5, extra);
        let result = validate_history_summarizer_output(
            &text,
            &aliased_chunk(1, 4),
            &[],
            ValidateOptions::default(),
        )
        .expect("discard-last should still make forward progress");

        assert!(result.discarded_last);
        // Facts are not anchor-filtered: the citation into the discarded segment (`s3`, ordinal 3) is a recorded rejection of the whole set.
        assert_eq!(
            result.extraction,
            ExtractionOutcome::Rejected {
                failure: ExtractionFailure::OutsideAcceptedSegment
            }
        );
        assert!(result.facts.is_empty());
        assert!(result.events.is_empty());
        assert!(result.user_observations.is_empty());
        assert!(result.primer_candidates.is_empty());
    }

    #[test]
    fn force_keep_last_preserves_final_history_segment_and_side_channels() {
        let extra = r#"
<events>
<trajectory_correction at_history_segment="1"><summary>earlier event</summary></trajectory_correction>
<trajectory_correction at_history_segment="2"><summary>final event</summary></trajectory_correction>
</events>
"#;
        let text = xml(&[(1, 2, "first"), (3, 4, "final")], 5, extra);
        let options = ValidateOptions {
            force_keep_last_history_segment: true,
            ..ValidateOptions::default()
        };
        let result = validate_history_summarizer_output(&text, &chunk(1, 4), &[], options)
            .expect("force-keep wrapup output should validate");

        assert!(!result.discarded_last);
        assert_eq!(result.history_segments.len(), 2);
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].at_history_segment, Some(1));
    }
}

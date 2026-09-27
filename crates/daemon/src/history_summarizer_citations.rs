//! Frozen native aliases for History Summarizer fact citations (Q30).
//!
//! Every message part the chunk renders carries an alias `sN` that names the native message, the blocks whose text produced the part, their content hashes, and the exact presented bytes. A fact cites `[sN:start-end]`, a half-open UTF-8 byte range inside that presented text; validation accepts a citation only when the alias exists, the range lies on character boundaries inside the presented bytes, and the alias's message falls inside the finally accepted history segment. The table travels with the chunk; a reattachment rebuilds it from the same frozen messages under the same budget, which the builder's tests hold byte-identical.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::history_summarizer_validate::FactCandidate;

/// Aliases a chunk may issue; a chunk renders far fewer messages than this.
pub const MAX_ALIASES: usize = 4096;
/// Cited spans per fact, at most; the Kernel refuses a staged subject past the same bound.
pub const MAX_CITATIONS_PER_FACT: usize = kernel::MAX_FACT_SPANS;
/// Facts per set, at most; the Kernel refuses a staged subject past the same bound.
pub const MAX_FACTS_PER_SET: usize = kernel::MAX_REVIEW_FACTS;
/// Claims kept per history segment, at most; the prompt states the same bounds.
pub const CLAIMS_PER_SEGMENT: usize = 8;
/// A claim key's bytes, at most.
pub const CLAIM_KEY_MAX_BYTES: usize = 64;
/// A claim value's bytes after XML unescaping, at most.
pub const CLAIM_VALUE_MAX_BYTES: usize = 128;
/// A claim anchor's bytes after XML unescaping, at most.
pub const CLAIM_ANCHOR_MAX_BYTES: usize = 200;

/// One presented message part and the native identity behind it. `alias` is empty until [`FrozenAliasTable::issue`] assigns it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenAlias {
    pub alias: String,
    /// Native message id and ordinal the part was rendered from.
    pub message_id: String,
    pub ordinal: u64,
    /// Native flat-block ids whose text contributed to the part, in block order. Tool-only parts name the tool blocks their summaries stand for.
    pub block_ids: Vec<String>,
    /// Lower-hex SHA-256 of each contributing block's serialized bytes, aligned with `block_ids`.
    pub block_hashes: Vec<String>,
    /// The exact bytes presented to the model for this part, after compaction and any truncation.
    pub presented: String,
    /// Whether the presented text is a transformation of the native text (compaction, truncation, tool summary, marker escaping) rather than a verbatim copy.
    pub transformed: bool,
}

/// The chunk's alias table, in issue order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenAliasTable {
    pub aliases: Vec<FrozenAlias>,
}

impl FrozenAliasTable {
    /// Assigns the next alias to `part` and returns its name. Beyond [`MAX_ALIASES`] parts the table is full: the part is rendered without a marker and cannot be cited.
    pub fn issue(&mut self, mut part: FrozenAlias) -> Option<String> {
        if self.aliases.len() >= MAX_ALIASES {
            return None;
        }
        part.alias = format!("s{}", self.aliases.len() + 1);
        let alias = part.alias.clone();
        self.aliases.push(part);
        Some(alias)
    }

    pub fn resolve(&self, alias: &str) -> Option<&FrozenAlias> {
        self.aliases.iter().find(|frozen| frozen.alias == alias)
    }
}

/// One citation as a fact carries it: an alias and a half-open byte range inside its presented text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    pub alias: String,
    pub start: usize,
    pub end: usize,
}

pub use memory_store::ExtractionFailure;

/// The extraction result that accompanies independently validated history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtractionOutcome {
    /// The run was extraction-free or memory-disabled: no fact material was requested or emitted.
    #[default]
    NotRequested,
    /// The model emitted an empty fact set on purpose.
    NoFacts,
    /// Every proposed fact and citation checked; these are the accepted facts.
    Accepted { count: usize },
    /// At least one proposed fact or citation failed; the whole set is rejected and history advances alone.
    Rejected { failure: ExtractionFailure },
}

/// Parses the citation prefix of one fact item: `[s3:12-40][s4:0-9] text`. Returns the citations and the remaining text; an item with no prefix has no citations, and a bracket that is not alias-shaped (`[sN:` with at least one digit) belongs to the text. An alias-shaped bracket is judged strictly: its range must be two runs of ASCII digits joined by `-`, and it must close.
pub fn split_citations(item: &str) -> Result<(Vec<Citation>, &str), ExtractionFailure> {
    let mut rest = item.trim_start();
    let mut citations = Vec::new();
    while let Some(after_open) = rest.strip_prefix('[') {
        let Some((alias, _)) = after_open.split_once(':') else {
            break;
        };
        if !is_alias_shaped(alias) {
            break;
        }
        let Some(close) = after_open.find(']') else {
            return Err(ExtractionFailure::MalformedCitation);
        };
        // A shaped alias is digits only, so the first `:` lies before the `]` and the range is the rest of the body.
        let range = &after_open[alias.len() + 1..close];
        let Some((start, end)) = range.split_once('-') else {
            return Err(ExtractionFailure::MalformedCitation);
        };
        let (Some(start), Some(end)) = (parse_offset(start), parse_offset(end)) else {
            return Err(ExtractionFailure::MalformedCitation);
        };
        citations.push(Citation {
            alias: alias.to_string(),
            start,
            end,
        });
        if citations.len() > MAX_CITATIONS_PER_FACT {
            return Err(ExtractionFailure::TooManyCitations);
        }
        rest = after_open[close + 1..].trim_start();
    }
    Ok((citations, rest))
}

/// `s` followed by one or more ASCII digits: the only alias shape the table issues and the shape the TypeScript reader recognizes.
fn is_alias_shaped(alias: &str) -> bool {
    alias.strip_prefix('s').is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// A byte offset as a run of ASCII digits; `usize::from_str` alone would also admit a leading `+`.
fn parse_offset(text: &str) -> Option<usize> {
    (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// Checks every fact's citations against the alias table and the ordinals of the finally accepted segments. Any failure rejects the whole set (Q30); a set with no failure is accepted whole.
pub fn check_fact_set(
    facts: &[FactCandidate],
    table: &FrozenAliasTable,
    accepted: Option<RangeInclusive<u64>>,
) -> Result<(), ExtractionFailure> {
    if facts.len() > MAX_FACTS_PER_SET {
        return Err(ExtractionFailure::TooManyFacts);
    }
    for fact in facts {
        if fact.citations.is_empty() {
            return Err(ExtractionFailure::MissingCitation);
        }
        if fact.citations.len() > MAX_CITATIONS_PER_FACT {
            return Err(ExtractionFailure::TooManyCitations);
        }
        for citation in &fact.citations {
            let frozen = table
                .resolve(&citation.alias)
                .ok_or(ExtractionFailure::UnknownAlias)?;
            match frozen.presented.get(citation.start..citation.end) {
                Some(span) if !span.is_empty() => {}
                _ => return Err(ExtractionFailure::InvalidSpan),
            }
            if !accepted
                .as_ref()
                .is_some_and(|range| range.contains(&frozen.ordinal))
            {
                return Err(ExtractionFailure::OutsideAcceptedSegment);
            }
        }
    }
    Ok(())
}

/// One `<claim>` as the summarizer emitted it, unescaped, before validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimCandidate {
    pub key: String,
    /// The `<cite>` text, parsed by [`check_claim_set`].
    pub cite: String,
    pub value: String,
    pub anchor: Option<String>,
}

/// The claims block's own verdict, independent of the segments and the facts:
/// no block, an unreadable block, or the claims kept after the per-claim rules.
/// `dropped` counts every emitted claim not kept: a rule failure, a claim a
/// later one with its key superseded, or one past the per-segment cap.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClaimsOutcome {
    #[default]
    NotRequested,
    Rejected {
        failure: ExtractionFailure,
    },
    Accepted {
        kept: usize,
        dropped: usize,
        anchor_missing: usize,
    },
}

/// The D-7 claim-key grammar `[a-z0-9_-]+(\.[a-z0-9_-]+)+` within the byte bound.
fn is_claim_key(key: &str) -> bool {
    let part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    };
    key.len() <= CLAIM_KEY_MAX_BYTES && key.contains('.') && key.split('.').all(part)
}

/// Validates claims against the chunk's alias table and the accepted
/// segments, in order: key grammar; value bound; one citation that resolves
/// through the frozen alias table to a span of presented bytes whose message
/// lies inside an accepted segment's range, with the value inside that span;
/// an anchor within its bound that the segment's trimmed `p1` contains (else
/// no anchor). A claim failing a rule before the anchor is dropped. Each
/// segment then keeps the last claim per key and the first
/// [`CLAIMS_PER_SEGMENT`] of those, in emitted order; a claim's position is
/// its index. Returns the claims per segment, aligned with `segments`, and the
/// counts.
pub fn check_claim_set(
    candidates: &[ClaimCandidate],
    table: &FrozenAliasTable,
    segments: &[(RangeInclusive<u64>, &str)],
) -> (Vec<Vec<memory_store::Claim>>, ClaimsOutcome) {
    let mut per_segment: Vec<Vec<memory_store::Claim>> = vec![Vec::new(); segments.len()];
    for candidate in candidates {
        if !is_claim_key(&candidate.key) || candidate.value.len() > CLAIM_VALUE_MAX_BYTES {
            continue;
        }
        let Ok((citations, rest)) = split_citations(&candidate.cite) else {
            continue;
        };
        let [citation] = citations.as_slice() else {
            continue;
        };
        if !rest.trim().is_empty() {
            continue;
        }
        let Some(frozen) = table.resolve(&citation.alias) else {
            continue;
        };
        if !frozen
            .presented
            .get(citation.start..citation.end)
            .is_some_and(|span| span.contains(candidate.value.as_str()))
        {
            continue;
        }
        let Some(index) = segments
            .iter()
            .position(|(range, _)| range.contains(&frozen.ordinal))
        else {
            continue;
        };
        let p1 = segments[index].1.trim();
        let anchor = candidate.anchor.clone().filter(|anchor| {
            !anchor.is_empty() && anchor.len() <= CLAIM_ANCHOR_MAX_BYTES && p1.contains(anchor)
        });
        let claims = &mut per_segment[index];
        // The last claim for a key wins: an earlier one leaves the order.
        claims.retain(|claim| claim.key != candidate.key);
        claims.push(memory_store::Claim {
            key: candidate.key.clone(),
            value: candidate.value.clone(),
            ordinal: frozen.ordinal as i64,
            anchor,
        });
    }
    for claims in &mut per_segment {
        claims.truncate(CLAIMS_PER_SEGMENT);
    }
    let kept: usize = per_segment.iter().map(Vec::len).sum();
    let anchor_missing = per_segment
        .iter()
        .flatten()
        .filter(|claim| claim.anchor.is_none())
        .count();
    let outcome = ClaimsOutcome::Accepted {
        kept,
        dropped: candidates.len() - kept,
        anchor_missing,
    };
    (per_segment, outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn fact(citations: Vec<Citation>) -> FactCandidate {
        FactCandidate {
            category: "PROJECT_RULES".into(),
            content: "rule".into(),
            citations,
        }
    }

    fn table(presented: &str) -> FrozenAliasTable {
        let mut table = FrozenAliasTable::default();
        table.issue(FrozenAlias {
            ordinal: 1,
            presented: presented.into(),
            ..FrozenAlias::default()
        });
        table
    }

    fn render(citations: &[Citation]) -> String {
        citations
            .iter()
            .map(|citation| format!("[{}:{}-{}]", citation.alias, citation.start, citation.end))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn citation_strategy() -> impl Strategy<Value = Citation> {
        (1usize..40, 0usize..200, 0usize..200).prop_map(|(n, start, end)| Citation {
            alias: format!("s{n}"),
            start,
            end,
        })
    }

    proptest! {
        /// A span is accepted exactly when `str::get` yields a non-empty slice: the independent boundary oracle.
        #[test]
        fn span_check_agrees_with_str_get(presented in "\\PC{0,24}", start in 0usize..40, end in 0usize..40) {
            let table = table(&presented);
            let outcome = check_fact_set(
                &[fact(vec![Citation { alias: "s1".into(), start, end }])],
                &table,
                Some(1..=1),
            );
            let expected = match presented.get(start..end) {
                Some(span) if !span.is_empty() => Ok(()),
                _ => Err(ExtractionFailure::InvalidSpan),
            };
            prop_assert_eq!(outcome, expected);
        }

        /// Rendering citations before free text and splitting again returns the same citations and the same text, for any text that does not itself start with a citation-shaped bracket.
        #[test]
        fn split_round_trips_rendered_citations(
            citations in prop::collection::vec(citation_strategy(), 0..=MAX_CITATIONS_PER_FACT),
            text in "[^\\[\\s][^\\r\\n]{0,20}",
        ) {
            let item = format!("{} {}", render(&citations), text);
            let (parsed, rest) = split_citations(&item).unwrap();
            prop_assert_eq!(parsed, citations);
            prop_assert_eq!(rest, text.as_str());
        }
    }

    fn claim(key: &str, cite: &str, value: &str, anchor: Option<&str>) -> ClaimCandidate {
        ClaimCandidate {
            key: key.into(),
            cite: cite.into(),
            value: value.into(),
            anchor: anchor.map(Into::into),
        }
    }

    proptest! {
        /// A cite is accepted exactly when the cited range is a span of the presented bytes that contains the value.
        #[test]
        fn cite_acceptance_is_value_inside_the_cited_span(
            presented in "[ab\u{e9}]{0,24}",
            value_range in (0usize..30, 0usize..30),
            other in "[ab]{0,3}",
            start in 0usize..30,
            end in 0usize..30,
        ) {
            // The value is usually a slice of the presented text, so it lies in the
            // message but often outside the cited span: the fault a whole-message check hides.
            let value = presented.get(value_range.0..value_range.1).unwrap_or(&other).to_string();
            let (claims, _) = check_claim_set(
                &[claim("k.v", &format!("[s1:{start}-{end}]"), &value, None)],
                &table(&presented),
                &[(1..=1, "")],
            );
            let expected = presented.get(start..end).is_some_and(|span| span.contains(value.as_str()));
            prop_assert_eq!(claims[0].len() == 1, expected);
        }
    }

    #[test]
    fn claims_constants_match_the_prompt_fixture() {
        let prompt = crate::history_summarizer_prompt::HISTORY_SUMMARIZER_SYSTEM_PROMPT;
        let section = &prompt[prompt
            .find("## Claims")
            .expect("the prompt has a claims section")..];
        let rule = |prefix: &str| {
            section
                .lines()
                .find(|line| line.starts_with(prefix))
                .unwrap_or_else(|| panic!("no rule {prefix}"))
        };
        assert!(rule("- `<key>`:").contains(&format!("at most {CLAIM_KEY_MAX_BYTES} bytes")));
        assert!(rule("- `<value>`:").contains(&format!("at most {CLAIM_VALUE_MAX_BYTES} bytes")));
        assert!(rule("- `<anchor>`:").contains(&format!("at most {CLAIM_ANCHOR_MAX_BYTES} bytes")));
        assert!(rule("- At most").starts_with(&format!(
            "- At most {CLAIMS_PER_SEGMENT} claims per history_segment."
        )));
    }

    #[test]
    fn each_claim_rule_drops_only_its_own_claim() {
        let long = "x".repeat(200) + "<";
        let table = table(&long);
        let p1 = format!("  {long}  ");
        let segments = [(1..=1, p1.as_str())];
        let value_128 = "x".repeat(127) + "<";
        let value_129 = "x".repeat(128) + "<";
        let cite = "[s1:0-201]";
        let (claims, outcome) = check_claim_set(
            &[
                claim("a.kept", cite, &value_128, Some("xx<")),
                claim("b.long", cite, &value_129, None),
                claim("Upper.case", cite, "x", None),
                claim("nodot", cite, "x", None),
                claim("c.absent", cite, "y", None),
                claim("d.two-cites", "[s1:0-1] [s1:0-2]", "x", None),
                claim("e.unknown-alias", "[s2:0-1]", "x", None),
                claim("f.anchor", cite, "x", Some("not in p1")),
                claim("g.long-anchor", cite, "x", Some(&"x".repeat(201))),
                claim("h.retracted", cite, "", Some("x")),
                // Only the trimmed p1 counts: its surrounding spaces are no anchor.
                claim("i.untrimmed", cite, "x", Some(" x")),
            ],
            &table,
            &segments,
        );
        let kept: Vec<_> = claims[0]
            .iter()
            .map(|claim| (claim.key.as_str(), claim.anchor.as_deref()))
            .collect();
        assert_eq!(
            kept,
            [
                ("a.kept", Some("xx<")),
                ("f.anchor", None),
                ("g.long-anchor", None),
                ("h.retracted", Some("x")),
                ("i.untrimmed", None),
            ]
        );
        assert_eq!(
            outcome,
            ClaimsOutcome::Accepted {
                kept: 5,
                dropped: 6,
                anchor_missing: 3,
            }
        );
    }

    #[test]
    fn a_segment_keeps_the_last_claim_per_key_then_the_first_eight() {
        let table = table("v");
        let mut candidates: Vec<_> = (0..9)
            .map(|index| claim(&format!("k.{index}"), "[s1:0-1]", "v", None))
            .collect();
        candidates.insert(1, claim("k.8", "[s1:0-1]", "v", Some("early")));
        candidates.push(claim("k.0", "[s1:0-1]", "v", Some("v")));
        let (claims, outcome) = check_claim_set(&candidates, &table, &[(1..=1, "v")]);
        let keys: Vec<_> = claims[0].iter().map(|claim| claim.key.as_str()).collect();
        assert_eq!(
            keys,
            ["k.1", "k.2", "k.3", "k.4", "k.5", "k.6", "k.7", "k.8"]
        );
        assert_eq!(claims[0][7].anchor, None, "the later k.8 wins");
        assert_eq!(
            outcome,
            ClaimsOutcome::Accepted {
                kept: 8,
                dropped: 3,
                anchor_missing: 8,
            }
        );
    }

    #[test]
    fn nine_citations_are_too_many_in_both_the_parser_and_the_checker() {
        let citations: Vec<Citation> = (0..9)
            .map(|index| Citation {
                alias: "s1".into(),
                start: index,
                end: index + 1,
            })
            .collect();
        assert_eq!(
            split_citations(&format!("{} text", render(&citations))),
            Err(ExtractionFailure::TooManyCitations)
        );
        assert_eq!(
            check_fact_set(&[fact(citations)], &table("0123456789"), Some(1..=1)),
            Err(ExtractionFailure::TooManyCitations)
        );
        let eight: Vec<Citation> = (0..8)
            .map(|index| Citation {
                alias: "s1".into(),
                start: index,
                end: index + 1,
            })
            .collect();
        assert_eq!(
            split_citations(&format!("{} text", render(&eight)))
                .unwrap()
                .0
                .len(),
            8
        );
        assert_eq!(
            check_fact_set(&[fact(eight)], &table("0123456789"), Some(1..=1)),
            Ok(())
        );
    }

    #[test]
    fn a_set_past_the_fact_bound_is_rejected_whole() {
        let one = || {
            fact(vec![Citation {
                alias: "s1".into(),
                start: 0,
                end: 1,
            }])
        };
        let at_bound: Vec<FactCandidate> = (0..MAX_FACTS_PER_SET).map(|_| one()).collect();
        assert_eq!(
            check_fact_set(&at_bound, &table("0123456789"), Some(1..=1)),
            Ok(())
        );
        let over: Vec<FactCandidate> = (0..=MAX_FACTS_PER_SET).map(|_| one()).collect();
        assert_eq!(
            check_fact_set(&over, &table("0123456789"), Some(1..=1)),
            Err(ExtractionFailure::TooManyFacts)
        );
    }

    #[test]
    fn malformed_shapes_and_non_citation_brackets() {
        // Alias-shaped bodies are judged strictly: a bad range or an unclosed alias-shaped bracket is malformed.
        for item in [
            "[s1:0-x] a",
            "[s1:0] a",
            "[s1:0-5 a",
            "[s1:-] a",
            "[s1:+5-9] a",
            "[s1: 0-5] a",
        ] {
            assert_eq!(
                split_citations(item),
                Err(ExtractionFailure::MalformedCitation),
                "{item}"
            );
        }
        // Brackets that are not alias-shaped belong to the text, exactly as the TypeScript reader keeps them.
        for item in [
            "[note] keep me",
            "[x1:0-2] not an alias",
            "[s:0-5] no digits is not an alias",
            "[unclosed bracket stays text",
        ] {
            assert_eq!(split_citations(item), Ok((Vec::new(), item)), "{item}");
        }
        assert_eq!(
            split_citations("[s1:0-2] [note] keep me").unwrap().1,
            "[note] keep me"
        );
    }

    #[test]
    fn the_table_stops_issuing_at_its_cap() {
        let mut table = FrozenAliasTable::default();
        for _ in 0..MAX_ALIASES {
            assert!(table.issue(FrozenAlias::default()).is_some());
        }
        assert_eq!(table.issue(FrozenAlias::default()), None);
        assert_eq!(table.aliases.len(), MAX_ALIASES);
        assert_eq!(
            table.aliases.last().map(|alias| alias.alias.as_str()),
            Some("s4096")
        );
        assert!(table.resolve("s4097").is_none());
    }
}

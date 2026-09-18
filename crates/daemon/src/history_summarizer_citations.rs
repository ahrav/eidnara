//! Frozen native aliases for History Summarizer fact citations (Q30).
//!
//! Every message part the chunk renders carries an alias `sN` that names the native message, the blocks whose text produced the part, their content hashes, and the exact presented bytes. A fact cites `[sN:start-end]`, a half-open UTF-8 byte range inside that presented text; validation accepts a citation only when the alias exists, the range lies on character boundaries inside the presented bytes, and the alias's message falls inside the finally accepted history segment. The table travels with the chunk; a reattachment rebuilds it from the same frozen messages under the same budget, which the builder's tests hold byte-identical.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::history_summarizer_validate::FactCandidate;

/// Aliases a chunk may issue; a chunk renders far fewer messages than this.
pub const MAX_ALIASES: usize = 4096;
/// Cited spans per fact, at most.
pub const MAX_CITATIONS_PER_FACT: usize = 8;

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

/// Why a proposed fact set was rejected as a whole. Host-authored, closed, and content-free: the code is what #595 records as the nonadmission reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionFailure {
    /// A fact cited an alias the chunk never issued.
    #[error("unknown_alias")]
    UnknownAlias,
    /// A cited range is empty, inverted, past the presented text, or not on UTF-8 character boundaries.
    #[error("invalid_span")]
    InvalidSpan,
    /// A citation names a message outside the finally accepted history segment.
    #[error("outside_accepted_segment")]
    OutsideAcceptedSegment,
    /// A fact carries more citations than [`MAX_CITATIONS_PER_FACT`].
    #[error("too_many_citations")]
    TooManyCitations,
    /// A citation's syntax did not parse, or a fact item was citations with no text.
    #[error("malformed_citation")]
    MalformedCitation,
    /// The `<facts>` block carried material that is neither a category block nor a bullet item, or appeared more than once.
    #[error("malformed_facts")]
    MalformedFacts,
    /// A fact carried no citation, so it cannot be reattached to native identity.
    #[error("missing_citation")]
    MissingCitation,
}

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

/// Parses the citation prefix of one fact item: `[s3:12-40][s4:0-9] text`. Returns the citations and the remaining text; an item with no prefix has no citations, and a bracket that is not citation-shaped belongs to the text.
pub fn split_citations(item: &str) -> Result<(Vec<Citation>, &str), ExtractionFailure> {
    let mut rest = item.trim_start();
    let mut citations = Vec::new();
    while let Some(after_open) = rest.strip_prefix('[') {
        let Some(close) = after_open.find(']') else {
            return Err(ExtractionFailure::MalformedCitation);
        };
        let body = &after_open[..close];
        let Some((alias, range)) = body.split_once(':') else {
            break;
        };
        if !alias.starts_with('s')
            || !alias[1..]
                .chars()
                .all(|character| character.is_ascii_digit())
        {
            break;
        }
        let Some((start, end)) = range.split_once('-') else {
            return Err(ExtractionFailure::MalformedCitation);
        };
        let (Ok(start), Ok(end)) = (start.parse::<usize>(), end.parse::<usize>()) else {
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

/// Checks every fact's citations against the alias table and the ordinals of the finally accepted segments. Any failure rejects the whole set (Q30); a set with no failure is accepted whole.
pub fn check_fact_set(
    facts: &[FactCandidate],
    table: &FrozenAliasTable,
    accepted: Option<RangeInclusive<u64>>,
) -> Result<(), ExtractionFailure> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn fact(citations: Vec<Citation>) -> FactCandidate {
        FactCandidate {
            category: "PROJECT_RULES".into(),
            content: "rule".into(),
            origin_history_segment_index: None,
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
    fn malformed_shapes_and_non_citation_brackets() {
        for item in ["[s1:0-x] a", "[s1:0] a", "[s1:0-5 a", "[s1:-] a"] {
            assert_eq!(
                split_citations(item),
                Err(ExtractionFailure::MalformedCitation),
                "{item}"
            );
        }
        assert_eq!(
            split_citations("[note] keep me").unwrap(),
            (Vec::new(), "[note] keep me")
        );
        assert_eq!(
            split_citations("[s1:0-2] [note] keep me").unwrap().1,
            "[note] keep me"
        );
        assert_eq!(
            split_citations("[x1:0-2] not an alias").unwrap(),
            (Vec::new(), "[x1:0-2] not an alias")
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

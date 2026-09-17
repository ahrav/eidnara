//! Bounded, read-only discovery of related memories for a Curator run.
//!
//! Discovery walks the Kernel's live canonical-claim and promoted-memory descriptors at one reusable snapshot in keyset order (Q17), examines a bounded number of candidates per call, and keeps the ones whose text, the bytes their descriptor's span selects from the artifact, contains a term of the subject. Relatedness is tested through the broker's read-only probe, which judges eligibility, applies the egress verdict, and render-checks the whole artifact; a probe-only candidate receives no alias, hold, retained bytes, or charge. A matching candidate is disclosed through the broker's ordinary read path, so every hit is held, revalidated, tagged, charged, and added to the disclosed-input union. Discovery commits no state and records no read repair.
//!
//! A page ends with a run-local continuation cursor or an explicit completeness code; zero hits never prove absence, and a cursor the run did not issue is refused rather than restarting discovery. Excerpts follow Q19: a half-open byte span on UTF-8 boundaries into the referenced artifact, at most [`MAX_EXCERPT_BYTES`] long.

use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroUsize;
use std::ops::Range;

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{KernelStore, LiveDescriptor};

use super::broker::{Alias, EvidenceBroker, Probed, ReferenceExpectation, Refusal, RefusalCode};

/// Hits per page; one page fills at most one model batch.
pub const MAX_RELATED_PAGE_HITS: usize = 8;
/// Descriptor rows examined per call before an explicit incompleteness stop.
pub const MAX_RELATED_CANDIDATES_PER_PAGE: usize = 64;
/// Artifact bytes probed per call before an explicit incompleteness stop.
pub const MAX_PAGE_PROBE_BYTES: u64 = 4 * 1024 * 1024;
/// Longest artifact worth probing: the render check refuses anything longer, so a longer candidate is skipped before any byte is read.
pub const MAX_PROBE_ARTIFACT_BYTES: u64 = context_core::redaction::MAX_REDACTABLE_BYTES as u64;
/// Per-hit excerpt bound (Q19), in bytes of the referenced artifact.
pub const MAX_EXCERPT_BYTES: usize = 512;
/// Cursors a run keeps resolvable; the oldest is forgotten first.
pub const MAX_LIVE_CURSORS: usize = 64;
const MAX_QUERY_TERMS: usize = 32;
/// Shortest subject token used as a matcher; shorter tokens match too much to locate anything.
const MIN_TERM_BYTES: usize = 4;
/// Bytes kept before the first matching term so an excerpt carries its lead-in.
const EXCERPT_LEAD_BYTES: usize = 64;
/// Longest subject token used as a matcher: the excerpt after its lead must hold the whole term.
const MAX_TERM_BYTES: usize = MAX_EXCERPT_BYTES - EXCERPT_LEAD_BYTES;
const CURSOR_PREFIX: &str = "cur-";
const CLASSES: [OccurrenceClass; 2] = [
    OccurrenceClass::CanonicalClaims,
    OccurrenceClass::PromotedMemory,
];

/// Why a page stopped where it did. Only `Complete` means the inventory at the snapshot was exhausted; every other code comes with a cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completeness {
    /// Every candidate at the snapshot was examined.
    Complete,
    /// The candidate bound for this call was reached.
    CandidateBound,
    /// The probe byte bound for this call was reached.
    ProbeBound,
    /// The hit bound for this page was reached.
    PageFull,
    /// The model batch has no operation left, or a broker capacity bound stopped the page after at least one hit; the cursor resumes at the undelivered candidate.
    CapacityBound,
    /// The caller's budget expired or was cancelled after the page passed at least one candidate; the cursor resumes at the next.
    BudgetBound,
}

/// One related memory, disclosed through the broker.
#[derive(Debug)]
pub struct RelatedHit {
    pub alias: Alias,
    /// Half-open byte span of the excerpt inside the referenced artifact, on UTF-8 boundaries.
    pub span: Range<u64>,
    pub excerpt: Vec<u8>,
    /// The alias an earlier disclosure of the same originating decision was issued under, when there is one.
    pub shared_origin: Option<Alias>,
}

#[derive(Debug)]
pub struct RelatedPage {
    pub hits: Vec<RelatedHit>,
    pub next_cursor: Option<String>,
    pub completeness: Completeness,
    /// Whether the page examined at least one candidate it could not deliver: a malformed row, an artifact too large to probe, or one the broker refused. The count is not exposed.
    pub withheld: bool,
}

/// Where a page resumes: the snapshot tip, the class being walked, and the last object id delivered or skipped.
#[derive(Debug, Clone)]
struct Cursor {
    tip: i64,
    class_index: usize,
    after: Option<String>,
}

/// Run-scoped discovery state: the subject's matcher terms and the cursors this run has issued.
pub struct RelatedMemoryDiscovery {
    /// ASCII-lowercased subject tokens of [`MIN_TERM_BYTES`] to [`MAX_TERM_BYTES`]; a candidate is related when its text contains one.
    terms: Vec<String>,
    /// Issued cursors, oldest first; a cursor token not in this table was never issued by this run or has been forgotten.
    cursors: VecDeque<(String, Cursor)>,
    issued: usize,
}

impl RelatedMemoryDiscovery {
    pub fn new(subject_text: &str) -> Self {
        let mut terms: Vec<String> = subject_text
            .split(|character: char| !character.is_alphanumeric())
            .filter(|term| (MIN_TERM_BYTES..=MAX_TERM_BYTES).contains(&term.len()))
            .map(str::to_ascii_lowercase)
            .collect();
        terms.sort();
        terms.dedup();
        terms.truncate(MAX_QUERY_TERMS);
        Self {
            terms,
            cursors: VecDeque::new(),
            issued: 0,
        }
    }

    /// The subject tokens that act as matchers.
    pub fn terms(&self) -> &[String] {
        &self.terms
    }

    /// One page of related memories. A `None` cursor starts at the current tip; a cursor this run issued continues at its snapshot; any other cursor is refused. A subject with no matcher term has nothing to find, so it completes at once without probing anything.
    pub fn page(
        &mut self,
        store: &KernelStore,
        broker: &mut EvidenceBroker,
        cursor: Option<&str>,
        budget: &EvalBudget,
        now_ms: i64,
    ) -> Result<RelatedPage, Refusal> {
        let mut cursor = match cursor {
            Some(token) => self
                .cursors
                .iter()
                .find(|(issued, _)| issued == token)
                .map(|(_, cursor)| cursor.clone())
                .ok_or_else(|| refusal(RefusalCode::InvalidCursor))?,
            None => Cursor {
                tip: store
                    .tip_within_budget(budget)
                    .map_err(|_| refusal(RefusalCode::Store))?,
                class_index: 0,
                after: None,
            },
        };
        let mut hits = Vec::new();
        let mut withheld = false;
        let mut examined = 0usize;
        let mut probed = 0u64;
        let mut advanced = false;
        if self.terms.is_empty() {
            return Ok(self.finish(cursor, hits, withheld, Completeness::Complete));
        }
        loop {
            let remaining = MAX_RELATED_CANDIDATES_PER_PAGE.saturating_sub(examined);
            let Some(max_rows) = NonZeroUsize::new(remaining.min(MAX_RELATED_PAGE_HITS)) else {
                return Ok(self.finish(cursor, hits, withheld, Completeness::CandidateBound));
            };
            let page = match store.live_source_descriptors(
                CLASSES[cursor.class_index],
                cursor.tip,
                cursor.after.as_deref(),
                max_rows,
                budget,
            ) {
                Ok(page) => page,
                Err(kernel::KernelError::Deadline) if advanced => {
                    return Ok(self.finish(cursor, hits, withheld, Completeness::BudgetBound));
                }
                Err(_) => return Err(refusal(RefusalCode::Store)),
            };
            examined += page.rows.len();
            let revisions = decision_revisions(store, &page.rows)?;
            for row in &page.rows {
                // Every early return below leaves `cursor` before this row, so the next page examines it again; a row is only passed once it is delivered or skipped.
                // The store honors the budget only while fetching descriptors; probing and disclosing a candidate run on unbudgeted reads, so the deadline and cancellation are polled here, once per candidate. A budget spent after the page passed a candidate ends the page with what it has: the hits are already disclosed and charged, and the cursor keeps them from being disclosed again.
                if budget.check().is_err() {
                    if advanced {
                        return Ok(self.finish(cursor, hits, withheld, Completeness::BudgetBound));
                    }
                    return Err(refusal(RefusalCode::Store));
                }
                let step = match expectation(row, &revisions) {
                    None => Step::Skipped { probed_bytes: 0 },
                    Some(expectation) => {
                        let room = MAX_PAGE_PROBE_BYTES.saturating_sub(probed);
                        match self.render(store, broker, &expectation, room, now_ms) {
                            Ok(step) => step,
                            // A run bound reached after this page disclosed something ends the page with what it has; the broker's `read` has already recorded the partial disclosure for every capacity code.
                            Err(refusal) if refusal.code.is_capacity() && !hits.is_empty() => {
                                Step::Stop(Completeness::CapacityBound)
                            }
                            Err(refusal) => return Err(refusal),
                        }
                    }
                };
                match step {
                    Step::Hit { hit, probed_bytes } => {
                        probed += probed_bytes;
                        hits.push(hit);
                    }
                    Step::Unrelated { probed_bytes } => probed += probed_bytes,
                    Step::Skipped { probed_bytes } => {
                        probed += probed_bytes;
                        withheld = true;
                    }
                    Step::Stop(completeness) => {
                        if completeness == Completeness::CapacityBound && !advanced {
                            // Nothing passed and no room to disclose: the batch was spent before discovery could start, which the caller must know as a refusal, not as a page. A page that passed candidates keeps its cursor even without a hit, so refused reads that spent the batch are not examined again.
                            return Err(refusal(RefusalCode::BatchLimit));
                        }
                        return Ok(self.finish(cursor, hits, withheld, completeness));
                    }
                }
                cursor.after = Some(row.object_id.clone());
                advanced = true;
                if hits.len() >= MAX_RELATED_PAGE_HITS {
                    return Ok(self.finish(cursor, hits, withheld, Completeness::PageFull));
                }
            }
            // The store sets `next` only for a full page, so every continued iteration consumed `max_rows` rows and the walk cannot stall.
            if page.next.is_none() {
                if cursor.class_index + 1 < CLASSES.len() {
                    cursor.class_index += 1;
                    cursor.after = None;
                    advanced = true;
                } else {
                    return Ok(self.finish(cursor, hits, withheld, Completeness::Complete));
                }
            }
        }
    }

    /// Ends a page. A complete page carries no cursor; every other outcome registers `cursor` under a fresh run-local token.
    fn finish(
        &mut self,
        cursor: Cursor,
        hits: Vec<RelatedHit>,
        withheld: bool,
        completeness: Completeness,
    ) -> RelatedPage {
        let next_cursor = (completeness != Completeness::Complete).then(|| {
            self.issued += 1;
            let token = format!("{CURSOR_PREFIX}{}", self.issued);
            if self.cursors.len() >= MAX_LIVE_CURSORS {
                self.cursors.pop_front();
            }
            self.cursors.push_back((token.clone(), cursor));
            token
        });
        RelatedPage {
            hits,
            next_cursor,
            completeness,
            withheld,
        }
    }

    /// Probes one candidate for relatedness and, on a match, discloses one bounded excerpt around the first matching term. Nothing is issued, held, or charged for a candidate that does not match or cannot be probed within `room`, the page's remaining probe budget.
    fn render(
        &self,
        store: &KernelStore,
        broker: &mut EvidenceBroker,
        expectation: &ReferenceExpectation,
        room: u64,
        now_ms: i64,
    ) -> Result<Step, Refusal> {
        let max_bytes = room.min(MAX_PROBE_ARTIFACT_BYTES);
        let (bytes, offset) = match broker.probe_canonical_source(store, expectation, max_bytes) {
            Ok(Probed::Bytes { bytes, start }) => (bytes, start),
            // An artifact no page could probe is skipped; one this page has no room left for waits for the next page.
            Ok(Probed::TooLarge { byte_length }) if byte_length > MAX_PROBE_ARTIFACT_BYTES => {
                return Ok(Step::Skipped { probed_bytes: 0 });
            }
            Ok(Probed::TooLarge { .. }) => return Ok(Step::Stop(Completeness::ProbeBound)),
            // The bytes were read before the artifact was refused; they count against the page like any other probe.
            Ok(Probed::Refused { byte_length }) => {
                return Ok(Step::Skipped {
                    probed_bytes: byte_length,
                });
            }
            Err(refusal) if refusal.code.is_capacity() => return Err(refusal),
            Err(_) => return Ok(Step::Skipped { probed_bytes: 0 }),
        };
        let probed_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        // The render check already proved the artifact is UTF-8; ASCII folding keeps every byte offset in the folded copy equal to its offset in the artifact.
        let text = std::str::from_utf8(&bytes).map_err(|_| refusal(RefusalCode::Undecodable))?;
        let Some(span) = self.excerpt_span(text) else {
            return Ok(Step::Unrelated { probed_bytes });
        };
        // Disclosing consumes one batch operation; stopping here instead of provoking the refusal keeps the run's conclusions usable.
        if broker.accounting.batch_headroom() == 0 {
            return Ok(Step::Stop(Completeness::CapacityBound));
        }
        let alias = broker.aliases.issue(expectation.clone());
        // The probe returned the descriptor's selection; the disclosed span is located in the artifact.
        let span = offset + span.start as u64..offset + span.end as u64;
        let read = match broker.read(store, alias.as_str(), Some(span.clone()), now_ms) {
            Ok(read) => read,
            Err(refusal) if refusal.code.is_capacity() => return Err(refusal),
            Err(_) => return Ok(Step::Skipped { probed_bytes }),
        };
        let shared_origin = broker.shared_origin(alias.as_str())?.cloned();
        Ok(Step::Hit {
            hit: RelatedHit {
                alias,
                span,
                excerpt: read.buffer.bytes,
                shared_origin,
            },
            probed_bytes,
        })
    }

    /// The excerpt window around the first subject term in `text`, or `None` when no term occurs.
    fn excerpt_span(&self, text: &str) -> Option<Range<usize>> {
        let folded = text.to_ascii_lowercase();
        let position = self
            .terms
            .iter()
            .filter_map(|term| folded.find(term.as_str()))
            .min()?;
        // Rounding the lead up keeps at least `MAX_EXCERPT_BYTES - EXCERPT_LEAD_BYTES` bytes after the match, which bounds every matcher.
        let start = text.ceil_char_boundary(position.saturating_sub(EXCERPT_LEAD_BYTES));
        let end = text.floor_char_boundary(start.saturating_add(MAX_EXCERPT_BYTES));
        Some(start..end)
    }
}

/// What examining one candidate row produced.
enum Step {
    Hit {
        hit: RelatedHit,
        probed_bytes: u64,
    },
    Unrelated {
        probed_bytes: u64,
    },
    /// The row cannot be delivered by any page: malformed, too large to probe, or refused by the broker. Bytes read before the refusal still count against the page.
    Skipped {
        probed_bytes: u64,
    },
    /// The page ends before this row; the cursor resumes here.
    Stop(Completeness),
}

/// The live registry revision of every originating decision named by `rows`, so each expectation binds the revision discovery observed; a revision landing before disclosure is then refused as an origin change rather than judged against a guess.
fn decision_revisions(
    store: &KernelStore,
    rows: &[LiveDescriptor],
) -> Result<BTreeMap<String, i64>, Refusal> {
    let decisions: Vec<String> = rows
        .iter()
        .filter_map(|row| originating_decision(OccurrenceClass::from_code(&row.detail.class)?, row))
        .collect();
    let (_, states) = store
        .object_states(&decisions)
        .map_err(|_| refusal(RefusalCode::Store))?;
    Ok(decisions
        .into_iter()
        .zip(states)
        .filter_map(|(decision, state)| Some((decision, state?.object.source_revision)))
        .collect())
}

/// The broker expectation for one descriptor row, or `None` for a row without a known originating decision or with an unparsable revision.
fn expectation(
    row: &LiveDescriptor,
    revisions: &BTreeMap<String, i64>,
) -> Option<ReferenceExpectation> {
    let class = OccurrenceClass::from_code(&row.detail.class)?;
    let decision = originating_decision(class, row)?;
    let decision_source_revision = *revisions.get(&decision)?;
    Some(ReferenceExpectation::CanonicalSource {
        object_id: row.object_id.clone(),
        class,
        source_revision: row.detail.revision.parse().ok()?,
        artifact_digest: row.detail.artifact_digest.clone(),
        evidence_id: row.detail.evidence_id.clone(),
        originating_decision_id: decision,
        decision_source_revision,
    })
}

/// The decision a descriptor row derives from: the value of the class's leading identity field, which is the decision object id for every class in [`CLASSES`]. Reading the field name from the class keeps discovery in step with the Kernel's identity layout.
fn originating_decision(class: OccurrenceClass, row: &LiveDescriptor) -> Option<String> {
    let field = *class.identity_fields().first()?;
    row.detail
        .identity
        .iter()
        .find(|(name, _)| name == field)
        .map(|(_, value)| value.clone())
}

fn refusal(code: RefusalCode) -> Refusal {
    Refusal { alias: None, code }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_are_folded_deduplicated_and_bounded_below_by_length() {
        let discovery = RelatedMemoryDiscovery::new("Bun builds the Workspace; BUN, bun!");
        assert_eq!(discovery.terms(), ["builds", "workspace"]);
        assert!(RelatedMemoryDiscovery::new("a to be").terms().is_empty());
    }

    #[test]
    fn every_matcher_fits_inside_an_excerpt_after_its_lead() {
        let longest = "a".repeat(MAX_TERM_BYTES);
        let discovery = RelatedMemoryDiscovery::new(&format!("{longest} {longest}b"));
        assert_eq!(
            discovery.terms(),
            [longest.as_str()],
            "a token longer than an excerpt can carry is not a matcher"
        );
        // A multibyte lead whose cut falls inside a character: the excerpt must still reach the end of the term.
        let text = format!("{}x{longest}", "é".repeat(40));
        let span = discovery.excerpt_span(&text).unwrap();
        assert!(span.end - span.start <= MAX_EXCERPT_BYTES);
        assert!(text[span].contains(&longest));
    }

    #[test]
    fn excerpt_span_indexes_the_original_text_on_char_boundaries() {
        let discovery = RelatedMemoryDiscovery::new("workspace");
        let lead = "é".repeat(40);
        let text = format!("{lead}the WORKSPACE builds");
        let span = discovery.excerpt_span(&text).unwrap();
        assert!(text.is_char_boundary(span.start) && text.is_char_boundary(span.end));
        assert!(text[span.clone()].contains("WORKSPACE"));
        assert!(span.end - span.start <= MAX_EXCERPT_BYTES);
        let long = format!("{}workspace{}", "x".repeat(100), "y".repeat(1000));
        let span = discovery.excerpt_span(&long).unwrap();
        assert_eq!(span, 36..36 + MAX_EXCERPT_BYTES);
        assert!(discovery.excerpt_span("nothing here").is_none());
    }
}

use std::fmt;

use retrieval::fusion::OccurrenceId;
use retrieval::packing::{Group, MergedRange, TokenCount};

use super::ClaudeTokens;
use super::accounting::{AccountingProfile, Authority, Charge};
use crate::history_summarizer_prompt::escape_xml_content;

/// The widest window searched for a piece boundary when the anchor moves; a
/// single piece longer than this leaves the anchor where it is.
pub const DELTA_LOOKBACK_BYTES: usize = 2 * tokenizer::MAX_PIECE_BYTES;

/// The window searched first when the anchor moves.
const ANCHOR_LOOKBACK_BYTES: usize = 256;

/// Tail length past which the ledger moves its anchor forward.
const ANCHOR_ADVANCE_BYTES: usize = 4 * ANCHOR_LOOKBACK_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charged {
    BlockOpen,
    Required(OccurrenceId),
    GroupOpen(usize),
    Range(usize, usize),
    GroupClose(usize),
    BlockClose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerEntry {
    pub item: Charged,
    pub bytes: usize,
    pub charge: Charge,
}

/// Staged stores fragments priced against a ledger's current render but not
/// appended. `cost` is the sum of the entries' headroom-adjusted charges, so
/// a caller that deducts it and then commits charges exactly what it deducted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Staged {
    entries: Vec<(LedgerEntry, String)>,
    cost: Option<ClaudeTokens>,
    tail_tokens: ClaudeTokens,
}

impl Staged {
    /// `None` when the sum is not representable; nothing is saturated.
    pub(crate) fn cost(&self) -> Option<ClaudeTokens> {
        self.cost
    }
}

/// Each rendered byte is attributable to exactly one charged fragment.
#[derive(Clone, PartialEq, Eq)]
pub struct Ledger {
    profile: AccountingProfile,
    text: String,
    entries: Vec<LedgerEntry>,
    total: ClaudeTokens,
    total_with_headroom: ClaudeTokens,
    /// Byte offset from which piece scanning of `text` agrees with scanning of
    /// `text` plus any suffix; a heuristic profile pins it at zero.
    anchor: usize,
    /// The profile's count of `text[anchor..]`.
    tail_tokens: ClaudeTokens,
}

/// Payloads are never logged; the rendered byte length stands in for the
/// text.
impl fmt::Debug for Ledger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ledger")
            .field("profile", &self.profile)
            .field("rendered_bytes", &self.text.len())
            .field("entries", &self.entries)
            .field("total", &self.total)
            .field("total_with_headroom", &self.total_with_headroom)
            .finish_non_exhaustive()
    }
}

impl Ledger {
    pub fn open(profile: AccountingProfile) -> Self {
        let mut ledger = Self {
            profile,
            text: String::new(),
            entries: Vec::new(),
            total: ClaudeTokens::ZERO,
            total_with_headroom: ClaudeTokens::ZERO,
            anchor: 0,
            tail_tokens: ClaudeTokens::ZERO,
        };
        ledger.append(Charged::BlockOpen, BLOCK_OPEN_FRAGMENT);
        ledger
    }

    pub fn profile(&self) -> &AccountingProfile {
        &self.profile
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn entries(&self) -> &[LedgerEntry] {
        &self.entries
    }

    pub fn total(&self) -> ClaudeTokens {
        self.total
    }

    /// The budget-facing total: every charge with its authority's headroom.
    pub fn total_with_headroom(&self) -> ClaudeTokens {
        self.total_with_headroom
    }

    pub fn rendered_bytes(&self) -> usize {
        self.text.len()
    }

    /// Bytes from the anchor to the end of the render: what the next delta
    /// tokenizes besides its fragment.
    pub fn tail_bytes(&self) -> usize {
        self.text.len() - self.anchor
    }

    /// The charge `fragment` would add if appended now.
    pub fn delta(&self, fragment: &str) -> Charge {
        let mut scratch = self.text[self.anchor..].to_owned();
        let mut before = self.tail_tokens;
        self.price(&mut scratch, &mut before, fragment)
    }

    /// Advances `scratch` and `before` past `fragment`.
    fn price(&self, scratch: &mut String, before: &mut ClaudeTokens, fragment: &str) -> Charge {
        scratch.push_str(fragment);
        let after = self.profile.charge_uncached(scratch);
        let charge = after.less(*before);
        *before = after.tokens();
        charge
    }

    /// Prices `fragments` in order as if each were appended after the last,
    /// without changing the render. The exact tokenizer is local past a piece
    /// boundary, so each delta is taken over the tail from the anchor; a
    /// heuristic makes no such promise and is re-estimated over the whole
    /// render.
    pub(crate) fn stage(&self, fragments: impl IntoIterator<Item = (Charged, String)>) -> Staged {
        let mut scratch = self.text[self.anchor..].to_owned();
        let mut before = self.tail_tokens;
        let mut cost = Some(ClaudeTokens::ZERO);
        let entries = fragments
            .into_iter()
            .map(|(item, fragment)| {
                let charge = self.price(&mut scratch, &mut before, &fragment);
                cost = cost.and_then(|sum| sum.checked_add(charge.with_headroom()));
                let entry = LedgerEntry {
                    item,
                    bytes: fragment.len(),
                    charge,
                };
                (entry, fragment)
            })
            .collect();
        Staged {
            entries,
            cost,
            tail_tokens: before,
        }
    }

    /// Appends staged fragments at the charges they were priced at.
    pub(crate) fn commit(&mut self, staged: Staged) {
        for (entry, fragment) in staged.entries {
            self.text.push_str(&fragment);
            self.total = self
                .total
                .checked_add(entry.charge.tokens())
                .unwrap_or(ClaudeTokens::MAX);
            self.total_with_headroom = self
                .total_with_headroom
                .checked_add(entry.charge.with_headroom())
                .unwrap_or(ClaudeTokens::MAX);
            self.entries.push(entry);
        }
        self.tail_tokens = staged.tail_tokens;
        self.advance_anchor();
    }

    pub fn append(&mut self, item: Charged, fragment: &str) -> Charge {
        let staged = self.stage([(item, fragment.to_owned())]);
        let charge = staged.entries[0].0.charge;
        self.commit(staged);
        charge
    }

    pub fn close(&mut self) -> Charge {
        self.append(Charged::BlockClose, BLOCK_CLOSE_FRAGMENT)
    }

    /// Moves the anchor toward the end once the tail outgrows
    /// `ANCHOR_ADVANCE_BYTES`, so a delta tokenizes a short tail plus its
    /// fragment rather than the render.
    fn advance_anchor(&mut self) {
        if matches!(self.profile.authority(), Authority::Heuristic { .. }) {
            return;
        }
        if self.text.len() - self.anchor <= ANCHOR_ADVANCE_BYTES {
            return;
        }
        let Some(anchor) = [ANCHOR_LOOKBACK_BYTES, DELTA_LOOKBACK_BYTES]
            .into_iter()
            .map(|lookback| tokenizer::suffix_anchor(&self.text, lookback))
            .find(|&anchor| anchor > self.anchor)
        else {
            return;
        };
        self.anchor = anchor;
        self.tail_tokens = self.profile.charge_uncached(&self.text[anchor..]).tokens();
    }
}

pub const BLOCK_OPEN_FRAGMENT: &str = "<packed-context>\n";
pub const BLOCK_CLOSE_FRAGMENT: &str = "</packed-context>\n";

pub fn required_fragment(occurrence: OccurrenceId, bytes: &[u8]) -> String {
    format!(
        "<required id=\"{occurrence}\">\n{}\n</required>\n",
        escape_xml_content(&String::from_utf8_lossy(bytes))
    )
}

pub fn group_open_fragment(group: &Group) -> String {
    format!("<group first=\"{}\">\n", group.first_fused)
}

pub fn range_fragment(range: &MergedRange) -> String {
    format!(
        "<span start=\"{}\" end=\"{}\">{}</span>\n",
        range.span.start,
        range.span.end,
        escape_xml_content(&String::from_utf8_lossy(&range.bytes))
    )
}

pub const GROUP_CLOSE_FRAGMENT: &str = "</group>\n";

/// The ledger entries a group is charged as: the open wrapper, each range,
/// and the close wrapper, in render order. The wrapper is its own entries so
/// an adjustment that empties the group can reclaim it.
pub fn group_fragments(index: usize, group: &Group) -> Vec<(Charged, String)> {
    let mut fragments = Vec::with_capacity(group.ranges.len() + 2);
    fragments.push((Charged::GroupOpen(index), group_open_fragment(group)));
    for (position, range) in group.ranges.iter().enumerate() {
        fragments.push((Charged::Range(index, position), range_fragment(range)));
    }
    fragments.push((Charged::GroupClose(index), GROUP_CLOSE_FRAGMENT.to_owned()));
    fragments
}

/// The concatenation of [`group_fragments`].
pub fn group_fragment(group: &Group) -> String {
    group_fragments(0, group)
        .into_iter()
        .map(|(_, fragment)| fragment)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountingBound {
    RenderedBytes,
    EstimatedTokens,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountingExceeded {
    pub bound: AccountingBound,
    pub value: u64,
    pub limit: u64,
}

/// Every bound is caller-supplied; there is no default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountingBounds {
    pub max_rendered_bytes: usize,
    pub max_estimated_tokens: ClaudeTokens,
}

pub fn admit_render(ledger: &Ledger, bounds: &AccountingBounds) -> Result<(), AccountingExceeded> {
    if ledger.rendered_bytes() > bounds.max_rendered_bytes {
        return Err(AccountingExceeded {
            bound: AccountingBound::RenderedBytes,
            value: ledger.rendered_bytes() as u64,
            limit: bounds.max_rendered_bytes as u64,
        });
    }
    if ledger.total_with_headroom() > bounds.max_estimated_tokens {
        return Err(AccountingExceeded {
            bound: AccountingBound::EstimatedTokens,
            value: ledger.total_with_headroom().get(),
            limit: bounds.max_estimated_tokens.get(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_cache::{local_stats, test_cache_guard};

    #[test]
    fn ledger_deltas_bypass_the_shared_cache_and_tokenize_a_bounded_tail() {
        let _guard = test_cache_guard();
        let fragment =
            "<span start=\"10\" end=\"20\">fn main() { let value = compute(input); }</span>\n";
        let mut ledger = Ledger::open(AccountingProfile::exact_tokenizer());
        let before = local_stats();
        let mut shortest_after_lookback = usize::MAX;
        for position in 0..400 {
            ledger.append(Charged::Range(0, position), fragment);
            assert!(
                ledger.tail_bytes() <= ANCHOR_ADVANCE_BYTES + fragment.len(),
                "append {position}: tail {} outgrew the advance threshold",
                ledger.tail_bytes()
            );
            if ledger.rendered_bytes() > ANCHOR_ADVANCE_BYTES {
                shortest_after_lookback = shortest_after_lookback.min(ledger.tail_bytes());
            }
        }
        let after = local_stats();
        assert_eq!(
            after.calls, before.calls,
            "a ledger's sliding tails never enter the shared cache"
        );
        assert!(
            shortest_after_lookback <= 2 * ANCHOR_LOOKBACK_BYTES + fragment.len(),
            "the anchor moves close to the end once the tail outgrows the lookback: {shortest_after_lookback}"
        );
        assert_eq!(
            ledger.total().get(),
            tokenizer::estimate_tokens(ledger.text()) as u64,
            "re-anchoring preserves the whole-render total"
        );
    }
}

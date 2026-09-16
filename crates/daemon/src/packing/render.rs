use retrieval::fusion::OccurrenceId;
use retrieval::packing::{Group, MergedRange, TokenCount};

use super::ClaudeTokens;
use super::accounting::{AccountingProfile, Authority, Charge};
use crate::history_summarizer_prompt::escape_xml_content;

/// How far back the delta looks for a piece boundary before falling back to
/// the whole render.
pub const DELTA_LOOKBACK_BYTES: usize = 2 * tokenizer::MAX_PIECE_BYTES;

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

/// Each rendered byte is attributable to exactly one charged fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ledger {
    profile: AccountingProfile,
    text: String,
    entries: Vec<LedgerEntry>,
    total: ClaudeTokens,
    total_with_headroom: ClaudeTokens,
}

impl Ledger {
    pub fn open(profile: AccountingProfile) -> Self {
        let mut ledger = Self {
            profile,
            text: String::new(),
            entries: Vec::new(),
            total: ClaudeTokens::ZERO,
            total_with_headroom: ClaudeTokens::ZERO,
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

    /// The charge `fragment` would add if appended now. The exact tokenizer
    /// is local past a piece boundary, so its delta is taken over the tail
    /// from the last trustworthy boundary; a heuristic makes no such promise
    /// and is re-estimated over the whole render.
    pub fn delta(&self, fragment: &str) -> Charge {
        let start = match self.profile.authority() {
            Authority::Exact => tokenizer::suffix_anchor(&self.text, DELTA_LOOKBACK_BYTES),
            Authority::Heuristic { .. } => 0,
        };
        let tail = &self.text[start..];
        let before = self.profile.charge(tail).tokens();
        let after = self.profile.charge(&[tail, fragment].concat());
        after.less(before)
    }

    pub fn append(&mut self, item: Charged, fragment: &str) -> Charge {
        let charge = self.delta(fragment);
        self.text.push_str(fragment);
        self.total = self
            .total
            .checked_add(charge.tokens())
            .unwrap_or(ClaudeTokens::MAX);
        self.total_with_headroom = self
            .total_with_headroom
            .checked_add(charge.with_headroom())
            .unwrap_or(ClaudeTokens::MAX);
        self.entries.push(LedgerEntry {
            item,
            bytes: fragment.len(),
            charge,
        });
        charge
    }

    pub fn close(&mut self) -> Charge {
        self.append(Charged::BlockClose, BLOCK_CLOSE_FRAGMENT)
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

/// The whole group as one fragment, for the scan's marginal cost.
pub fn group_fragment(group: &Group) -> String {
    let mut fragment = group_open_fragment(group);
    for range in &group.ranges {
        fragment.push_str(&range_fragment(range));
    }
    fragment.push_str(GROUP_CLOSE_FRAGMENT);
    fragment
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

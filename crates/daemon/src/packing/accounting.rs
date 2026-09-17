use std::sync::Arc;

use crate::token_cache::{AccountingRevision, EXACT_TOKENIZER_IDENTITY, cached_count_under};

use super::ClaudeTokens;
use retrieval::packing::TokenCount;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    Exact,
    Heuristic {
        degradation: &'static str,
        headroom_permille: u32,
    },
}

/// Rendered bytes the packer does not charge; every other rendered byte
/// belongs to a ledger entry.
pub const DECLARED_UNCHARGED: &[&str] = &["separator-before-memory-block"];

#[derive(Clone)]
pub struct AccountingProfile {
    identity: &'static str,
    revision: AccountingRevision,
    authority: Authority,
    count: Arc<dyn Fn(&str) -> usize + Send + Sync>,
}

impl std::fmt::Debug for AccountingProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountingProfile")
            .field("identity", &self.identity)
            .field("revision", &self.revision.as_str())
            .field("authority", &self.authority)
            .finish_non_exhaustive()
    }
}

impl PartialEq for AccountingProfile {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity
            && self.revision == other.revision
            && self.authority == other.authority
    }
}

impl Eq for AccountingProfile {}

impl AccountingProfile {
    pub fn exact_tokenizer() -> Self {
        Self {
            identity: EXACT_TOKENIZER_IDENTITY,
            revision: AccountingRevision::exact_tokenizer().clone(),
            authority: Authority::Exact,
            count: Arc::new(tokenizer::estimate_tokens),
        }
    }

    /// The revision includes `identity` and `degradation`, so changing either
    /// invalidates cached counts.
    pub fn heuristic(
        identity: &'static str,
        degradation: &'static str,
        headroom_permille: u32,
        count: impl Fn(&str) -> usize + Send + Sync + 'static,
    ) -> Self {
        Self {
            identity,
            revision: AccountingRevision::heuristic(identity, degradation),
            authority: Authority::Heuristic {
                degradation,
                headroom_permille,
            },
            count: Arc::new(count),
        }
    }

    pub fn identity(&self) -> &'static str {
        self.identity
    }

    pub fn revision(&self) -> &AccountingRevision {
        &self.revision
    }

    pub fn authority(&self) -> Authority {
        self.authority
    }

    pub fn declared_uncharged(&self) -> &'static [&'static str] {
        DECLARED_UNCHARGED
    }

    pub fn charge(&self, text: &str) -> Charge {
        let count = cached_count_under(&self.revision, text, |text| (self.count)(text));
        self.charge_of(count)
    }

    /// Counts without the shared cache; the cache only pays off for text that
    /// recurs.
    pub fn charge_uncached(&self, text: &str) -> Charge {
        self.charge_of((self.count)(text))
    }

    fn charge_of(&self, count: usize) -> Charge {
        Charge {
            tokens: ClaudeTokens::new(count as u64),
            authority: self.authority,
        }
    }
}

/// ```compile_fail,E0451
/// let _ = daemon::packing::Charge {
///     tokens: daemon::packing::ClaudeTokens::new(1),
///     authority: daemon::packing::Authority::Exact,
/// };
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Charge {
    tokens: ClaudeTokens,
    authority: Authority,
}

impl Charge {
    pub fn tokens(self) -> ClaudeTokens {
        self.tokens
    }

    pub fn authority(self) -> Authority {
        self.authority
    }

    /// The render ledger's delta; a profile's count is the only other source
    /// of a charge, so an exact label never leaves the crate cheaper than it
    /// was counted.
    pub(crate) fn less(self, before: ClaudeTokens) -> Self {
        Self {
            tokens: self
                .tokens
                .checked_sub(before)
                .unwrap_or(ClaudeTokens::ZERO),
            authority: self.authority,
        }
    }

    /// The tokens plus the authority's headroom, rounded up.
    pub fn with_headroom(self) -> ClaudeTokens {
        let permille = match self.authority {
            Authority::Exact => 0,
            Authority::Heuristic {
                headroom_permille, ..
            } => u64::from(headroom_permille),
        };
        // The product is taken in `u128` so a ratio whose product exceeds
        // `u64` is still charged at that ratio; only the final sum saturates.
        let extra = (u128::from(self.tokens.get()) * u128::from(permille)).div_ceil(1_000);
        let extra = u64::try_from(extra).unwrap_or(u64::MAX);
        ClaudeTokens::new(self.tokens.get().saturating_add(extra))
    }
}

use memory_store::{FoldAuthorityRecord, MemoryStoreError, TruncateOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldAuthorityIntent {
    pub eidnara_folds: bool,
    pub admitted: bool,
    pub sibling_bound: bool,
    pub first_pass: bool,
}

impl Default for FoldAuthorityIntent {
    fn default() -> Self {
        Self {
            eidnara_folds: true,
            admitted: true,
            sibling_bound: false,
            first_pass: true,
        }
    }
}

pub trait SiblingFence: Sync {
    /// Runs `change` and returns its result while this binding is the session's sole binding;
    /// otherwise returns `None` and skips `change`.
    fn without_sibling(&self, change: &mut dyn FnMut() -> AuthorityReset)
    -> Option<AuthorityReset>;
}

pub type AuthorityReset = Result<Option<TruncateOutcome>, MemoryStoreError>;

pub struct NoSiblings;

impl SiblingFence for NoSiblings {
    fn without_sibling(
        &self,
        change: &mut dyn FnMut() -> AuthorityReset,
    ) -> Option<AuthorityReset> {
        Some(change())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingReason {
    SiblingBound,
    NotQuiescent,
    LaterBind,
}

impl PendingReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::SiblingBound => "another binding is open on this session",
            Self::NotQuiescent => "the summarizer is busy or a publication is pending",
            Self::LaterBind => "it applies at the next bind while the session is quiescent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingAuthority {
    pub target: bool,
    pub reason: PendingReason,
}

impl PendingAuthority {
    pub fn describe(self) -> String {
        format!(
            "fold authority pending {}: {}",
            authority_name(self.target),
            self.reason.as_str()
        )
    }
}

pub fn authority_name(eidnara_folds: bool) -> &'static str {
    if eidnara_folds { "eidnara" } else { "native" }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AppliedFoldAuthority {
    pub eidnara_folds: bool,
    pub pending: Option<PendingAuthority>,
    pub settles_first_pass: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldAuthorityPlan {
    pub eidnara_folds: bool,
    pub row_version: Option<u64>,
    pub adopt: Option<bool>,
    pub change: Option<bool>,
    pub pending: Option<PendingAuthority>,
}

pub fn plan(record: FoldAuthorityRecord, intent: FoldAuthorityIntent) -> FoldAuthorityPlan {
    let stay = |eidnara_folds: bool, adopt: Option<bool>, pending: Option<PendingAuthority>| {
        FoldAuthorityPlan {
            eidnara_folds,
            row_version: record.row_version,
            adopt,
            change: None,
            pending,
        }
    };
    let Some(applied) = record.applied else {
        return stay(
            intent.eidnara_folds,
            intent.admitted.then_some(intent.eidnara_folds),
            None,
        );
    };
    let adopt = (record.adopted.is_none() && intent.admitted).then_some(applied);
    if applied == intent.eidnara_folds || !intent.admitted {
        return stay(applied, adopt, None);
    }
    let reason = if record.adopted.is_none() || !intent.first_pass {
        PendingReason::LaterBind
    } else if intent.sibling_bound {
        PendingReason::SiblingBound
    } else if !record.quiescent {
        PendingReason::NotQuiescent
    } else {
        return FoldAuthorityPlan {
            eidnara_folds: applied,
            row_version: record.row_version,
            adopt: None,
            change: Some(intent.eidnara_folds),
            pending: None,
        };
    };
    stay(
        applied,
        adopt,
        Some(PendingAuthority {
            target: intent.eidnara_folds,
            reason,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        applied: Option<bool>,
        adopted: Option<bool>,
        quiescent: bool,
    ) -> FoldAuthorityRecord {
        FoldAuthorityRecord {
            row_version: Some(1),
            applied,
            adopted,
            quiescent,
        }
    }

    #[test]
    fn the_transition_table_follows_the_session_authority_rules() {
        for (applied, legacy) in [
            (None, false),
            (Some(false), false),
            (Some(true), false),
            (Some(true), true),
        ] {
            let adopted = if legacy { None } else { applied };
            for bits in 0..32u8 {
                let quiescent = bits & 1 != 0;
                let intent = FoldAuthorityIntent {
                    eidnara_folds: bits & 2 != 0,
                    admitted: bits & 4 != 0,
                    sibling_bound: bits & 8 != 0,
                    first_pass: bits & 16 != 0,
                };
                let got = plan(record(applied, adopted, quiescent), intent);
                let case = format!("{applied:?} legacy={legacy} {intent:?} quiescent={quiescent}");
                assert_eq!(got.row_version, Some(1), "{case}");
                let Some(stored) = applied else {
                    assert_eq!(got.eidnara_folds, intent.eidnara_folds, "{case}");
                    assert_eq!(
                        got.adopt,
                        intent.admitted.then_some(intent.eidnara_folds),
                        "{case}"
                    );
                    assert_eq!((got.change, got.pending), (None, None), "{case}");
                    continue;
                };
                assert_eq!(got.eidnara_folds, stored, "{case}");
                assert_eq!(
                    got.adopt,
                    (legacy && intent.admitted).then_some(true),
                    "{case}"
                );
                let disagrees = stored != intent.eidnara_folds && intent.admitted;
                let changes =
                    disagrees && !legacy && intent.first_pass && !intent.sibling_bound && quiescent;
                assert_eq!(
                    got.change,
                    changes.then_some(intent.eidnara_folds),
                    "{case}"
                );
                let reason = if legacy || !intent.first_pass {
                    PendingReason::LaterBind
                } else if intent.sibling_bound {
                    PendingReason::SiblingBound
                } else {
                    PendingReason::NotQuiescent
                };
                assert_eq!(
                    got.pending,
                    (disagrees && !changes).then_some(PendingAuthority {
                        target: intent.eidnara_folds,
                        reason,
                    }),
                    "{case}"
                );
            }
        }
    }
}

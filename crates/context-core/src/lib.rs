//! Chooses cache-pass work from caller-computed conversation state.
//!
//! Classification is independent of provider origin. This crate does not parse
//! provider bytes, render content, perform I/O, or inspect frozen units.

#![forbid(unsafe_code)]

pub mod canonical_json;
pub mod decay;
pub mod redaction;

/// Defaults to [`Unknown`](Self::Unknown) so a defaulted input is rejected rather than destructively rebuilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PersistedShape {
    /// A fresh shape selects bootstrap Hard before any defer can replay a baseline.
    Fresh,
    /// The shape contains one `"baseline"` frozen unit with no pending changes.
    /// Only the legacy baseline shape permits destructive clear-then-Hard migration.
    LegacyBaseline,
    /// The shape contains exactly one `m0`, exactly one `m1`, and zero or more `red:*` units.
    /// Only this shape proceeds to epoch and delta checks.
    ValidM0M1,
    /// The shape contains one valid `m0` and no `m1`.
    /// A missing m1 is a recoverable cache shape, not an unknown schema, so it is rebuilt with Hard rather than rejected.
    CachedM1Missing,
    /// An initialized shape that is neither legacy, `m1`-missing, nor valid.
    /// Unknown frozen-set shapes are rejected and never cleared.
    #[default]
    Unknown,
}

/// The consuming module computes every `ClassifierInput` field.
/// This crate receives decision inputs without inspecting frozen units.
#[derive(Debug, Clone, Default)]
pub struct ClassifierInput {
    pub shape: PersistedShape,
    /// Render-config (model/system/tool) differs from the persisted one → epoch Hard.
    pub render_config_changed: bool,
    /// A HARD trigger fired (compaction fold / idle-ttl / pressure) — decider-supplied.
    pub hard_fold_requested: bool,
    pub boundary_present: bool,
    /// `reconcile_pending` is true when an earlier defer loses the boundary.
    pub reconcile_pending: bool,
    /// `m1_revision_changed` compares revisions without rendering bytes.
    pub m1_revision_changed: bool,
    /// `reductions_pending` is true when a live-tail decision targets an ID absent from the frozen reduction set.
    /// ID membership is sufficient because frozen reductions are immutable.
    /// Within an epoch, only a previously unseen target ID changes the frozen reductions.
    /// `reductions_pending` and `m1_revision_changed` coalesce into one `Soft` pass when `boundary_present` and `bust_opportunity` are true.
    pub reductions_pending: bool,
    /// `bust_opportunity` is true only when an independent reason already renders bytes.
    /// Without `bust_opportunity`, an in-session m1 delta is deferred.
    pub bust_opportunity: bool,
}

/// Pass selected from persisted shape, epoch state, and pending deltas.
///
/// `Reject` identifies an unrecognized persisted shape and never authorizes
/// clearing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassPlan {
    /// Rebuild all rendered units.
    Hard,
    /// Clear the recognized legacy baseline, then rebuild all rendered units.
    MigrateHard,
    /// Re-render pending deltas during an independent bust opportunity.
    Soft,
    /// Preserve frozen units and postpone pending work.
    Defer,
    /// Refuse an unknown frozen-set shape without mutating it.
    Reject,
}

/// Selects one pass without rendering or mutating state.
///
/// Check order defines precedence. Bootstrap and recognized migration shapes
/// precede unknown-shape rejection. Hard triggers precede reconciliation, which
/// precedes soft deltas. This function does not panic or perform I/O.
pub fn classify(input: &ClassifierInput) -> PassPlan {
    match input.shape {
        PersistedShape::Fresh => return PassPlan::Hard,
        PersistedShape::LegacyBaseline => return PassPlan::MigrateHard,
        PersistedShape::CachedM1Missing => return PassPlan::Hard,
        PersistedShape::Unknown => return PassPlan::Reject,
        PersistedShape::ValidM0M1 => {}
    }
    if input.render_config_changed {
        return PassPlan::Hard;
    }
    if input.hard_fold_requested {
        return PassPlan::Hard;
    }
    if input.reconcile_pending && !input.boundary_present {
        return PassPlan::Hard;
    }
    if input.reconcile_pending {
        return PassPlan::Defer;
    }
    if input.boundary_present
        && input.bust_opportunity
        && (input.m1_revision_changed || input.reductions_pending)
    {
        return PassPlan::Soft;
    }
    PassPlan::Defer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ClassifierInput {
        ClassifierInput {
            shape: PersistedShape::ValidM0M1,
            boundary_present: true,
            bust_opportunity: true,
            ..Default::default()
        }
    }

    /// One row per shape and signal combination; the row order follows the
    /// precedence `classify` applies.
    #[test]
    fn classify_selects_the_pass_for_each_shape_and_signal_combination() {
        let cases = [
            (
                "a fresh shape bootstraps with Hard before any defer",
                ClassifierInput {
                    shape: PersistedShape::Fresh,
                    m1_revision_changed: true,
                    ..Default::default()
                },
                PassPlan::Hard,
            ),
            (
                "a legacy baseline migrates",
                ClassifierInput {
                    shape: PersistedShape::LegacyBaseline,
                    m1_revision_changed: true,
                    ..base()
                },
                PassPlan::MigrateHard,
            ),
            (
                "a missing m1 rebuilds with Hard",
                ClassifierInput {
                    shape: PersistedShape::CachedM1Missing,
                    ..base()
                },
                PassPlan::Hard,
            ),
            (
                "an unknown shape rejects and never clears",
                ClassifierInput {
                    shape: PersistedShape::Unknown,
                    m1_revision_changed: true,
                    ..base()
                },
                PassPlan::Reject,
            ),
            (
                "a defaulted input rejects",
                ClassifierInput::default(),
                PassPlan::Reject,
            ),
            (
                "a render-config change is Hard",
                ClassifierInput {
                    render_config_changed: true,
                    m1_revision_changed: true,
                    ..base()
                },
                PassPlan::Hard,
            ),
            (
                "a hard fold request is Hard",
                ClassifierInput {
                    hard_fold_requested: true,
                    m1_revision_changed: true,
                    ..base()
                },
                PassPlan::Hard,
            ),
            (
                "a pending reconcile with the boundary absent rematerializes",
                ClassifierInput {
                    reconcile_pending: true,
                    boundary_present: false,
                    m1_revision_changed: true,
                    ..base()
                },
                PassPlan::Hard,
            ),
            (
                "a pending reconcile with the boundary present defers even with a delta",
                ClassifierInput {
                    reconcile_pending: true,
                    boundary_present: true,
                    m1_revision_changed: true,
                    ..base()
                },
                PassPlan::Defer,
            ),
            (
                "an m1 delta rides a Soft when the boundary is present",
                ClassifierInput {
                    boundary_present: true,
                    m1_revision_changed: true,
                    ..base()
                },
                PassPlan::Soft,
            ),
            (
                "an m1 delta defers when the boundary is absent",
                ClassifierInput {
                    boundary_present: false,
                    m1_revision_changed: true,
                    reconcile_pending: false,
                    ..base()
                },
                PassPlan::Defer,
            ),
            (
                "an m1 delta without a bust opportunity defers",
                ClassifierInput {
                    m1_revision_changed: true,
                    bust_opportunity: false,
                    ..base()
                },
                PassPlan::Defer,
            ),
            (
                "a present boundary with no delta defers",
                ClassifierInput {
                    boundary_present: true,
                    m1_revision_changed: false,
                    reductions_pending: false,
                    ..base()
                },
                PassPlan::Defer,
            ),
            (
                "a new reduction rides a Soft",
                ClassifierInput {
                    boundary_present: true,
                    m1_revision_changed: false,
                    reductions_pending: true,
                    ..base()
                },
                PassPlan::Soft,
            ),
            (
                "an m1 delta and a reduction coalesce into one Soft",
                ClassifierInput {
                    boundary_present: true,
                    m1_revision_changed: true,
                    reductions_pending: true,
                    ..base()
                },
                PassPlan::Soft,
            ),
            (
                "a reduction with the boundary absent defers, never Soft",
                ClassifierInput {
                    boundary_present: false,
                    m1_revision_changed: true,
                    reductions_pending: true,
                    reconcile_pending: false,
                    ..base()
                },
                PassPlan::Defer,
            ),
        ];
        for (case, input, expected) in cases {
            assert_eq!(classify(&input), expected, "{case}: {input:?}");
        }
    }
}

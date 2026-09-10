//! Chooses cache-pass work from caller-computed conversation state.
//!
//! Classification is independent of provider origin. This crate does not parse
//! provider bytes, render content, perform I/O, or inspect frozen units.

#![forbid(unsafe_code)]

pub mod canonical_json;
pub mod decay;
pub mod redaction;

/// Defaults to [`Unknown`](Self::Unknown) so a defaulted input is rejected rather than destructively rebuilt. commentlint: allow(JUDGE)
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

    #[test]
    fn bootstrap_when_uninitialized_is_hard() {
        let input = ClassifierInput {
            shape: PersistedShape::Fresh,
            m1_revision_changed: true,
            ..Default::default()
        };
        assert_eq!(classify(&input), PassPlan::Hard);
    }

    #[test]
    fn legacy_baseline_migrates() {
        let input = ClassifierInput {
            shape: PersistedShape::LegacyBaseline,
            m1_revision_changed: true,
            ..base()
        };
        assert_eq!(classify(&input), PassPlan::MigrateHard);
    }

    #[test]
    fn missing_m1_is_rebuilt_as_hard() {
        let input = ClassifierInput {
            shape: PersistedShape::CachedM1Missing,
            ..base()
        };
        assert_eq!(classify(&input), PassPlan::Hard);
    }

    #[test]
    fn unknown_shape_rejects_never_clears() {
        let input = ClassifierInput {
            shape: PersistedShape::Unknown,
            m1_revision_changed: true,
            ..base()
        };
        assert_eq!(classify(&input), PassPlan::Reject);
    }

    #[test]
    fn defaulted_input_rejects() {
        assert_eq!(classify(&ClassifierInput::default()), PassPlan::Reject);
    }

    #[test]
    fn epoch_change_is_hard() {
        let input = ClassifierInput {
            render_config_changed: true,
            m1_revision_changed: true,
            ..base()
        };
        assert_eq!(classify(&input), PassPlan::Hard);
    }

    #[test]
    fn hard_fold_requested_is_hard() {
        let input = ClassifierInput {
            hard_fold_requested: true,
            m1_revision_changed: true,
            ..base()
        };
        assert_eq!(classify(&input), PassPlan::Hard);
    }

    #[test]
    fn reconcile_boundary_absent_rematerializes() {
        let input = ClassifierInput {
            reconcile_pending: true,
            boundary_present: false,
            m1_revision_changed: true,
            ..base()
        };
        assert_eq!(classify(&input), PassPlan::Hard);
    }

    #[test]
    fn reconcile_boundary_present_defers_to_clear() {
        let input = ClassifierInput {
            reconcile_pending: true,
            boundary_present: true,
            m1_revision_changed: true, // even with a delta, the clearing defer wins
            ..base()
        };
        assert_eq!(classify(&input), PassPlan::Defer);
    }

    #[test]
    fn soft_delta_rides_only_with_boundary_present() {
        let present = ClassifierInput {
            boundary_present: true,
            m1_revision_changed: true,
            ..base()
        };
        assert_eq!(classify(&present), PassPlan::Soft);

        let absent = ClassifierInput {
            boundary_present: false,
            m1_revision_changed: true,
            reconcile_pending: false,
            ..base()
        };
        assert_eq!(classify(&absent), PassPlan::Defer);
    }

    #[test]
    fn pending_delta_without_bust_opportunity_defers() {
        let input = ClassifierInput {
            m1_revision_changed: true,
            bust_opportunity: false,
            ..base()
        };
        assert_eq!(classify(&input), PassPlan::Defer);
    }

    #[test]
    fn boundary_present_no_delta_defers() {
        let input = ClassifierInput {
            boundary_present: true,
            m1_revision_changed: false,
            reductions_pending: false,
            ..base()
        };
        assert_eq!(classify(&input), PassPlan::Defer);
    }

    #[test]
    fn a_new_reduction_rides_a_soft() {
        let input = ClassifierInput {
            boundary_present: true,
            m1_revision_changed: false,
            reductions_pending: true,
            ..base()
        };
        assert_eq!(classify(&input), PassPlan::Soft);
    }

    #[test]
    fn m1_and_reduction_coalesce_into_one_soft() {
        let input = ClassifierInput {
            boundary_present: true,
            m1_revision_changed: true,
            reductions_pending: true,
            ..base()
        };
        assert_eq!(classify(&input), PassPlan::Soft);
    }

    #[test]
    fn boundary_absent_reduction_defers_never_soft() {
        let input = ClassifierInput {
            boundary_present: false,
            m1_revision_changed: true,
            reductions_pending: true,
            reconcile_pending: false,
            ..base()
        };
        assert_eq!(classify(&input), PassPlan::Defer);
    }
}

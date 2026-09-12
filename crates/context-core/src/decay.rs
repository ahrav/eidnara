//! Age-based paraphrase tiering.
//!
//! A compartment's tier follows its exponential decay position `z`, the age
//! measured in half-lives, where the half-life grows with importance and
//! shrinks with budget pressure. Tier boundaries are fixed so the same
//! compartment renders identically for a given index, importance, and
//! pressure.

/// Half-life in compartment positions for importance 50 at budget pressure 1.
pub const H50: f64 = 24.0;
/// Importance-point interval that doubles the half-life.
///
/// For example, importance 75 produces twice the baseline half-life and 100
/// produces four times the baseline half-life.
pub const D: f64 = 25.0;
/// Maximum anchor-overlap extension to the archive boundary, in half-lives.
pub const G: f64 = 2.0;

/// P1→P2 boundary.
pub const Z1: f64 = 0.201;
/// P2→P3 boundary.
pub const Z2: f64 = 0.729;
/// P3→P4 boundary.
pub const Z3: f64 = 1.322;
/// P4→P5 (archive candidate) boundary.
pub const Z4: f64 = 2.587;

/// Pressure floor: prevents div-by-zero and caps relaxation at 10×.
pub const P_FLOOR: f64 = 0.1;

/// A paraphrase tier. P5 denotes archival and is never rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    P1,
    P2,
    P3,
    P4,
    P5,
}

impl Tier {
    /// Estimated token cost of one compartment rendered at this tier.
    pub const fn cost(self) -> u32 {
        match self {
            Tier::P1 => 322,
            Tier::P2 => 109,
            Tier::P3 => 35,
            Tier::P4 => 20,
            Tier::P5 => 5,
        }
    }
}

#[inline]
fn z_value(compartment_index: u32, importance: i32, budget_pressure: f64) -> f64 {
    let a = (compartment_index.max(1) - 1) as f64;
    let imp = importance.clamp(1, 100) as f64;
    // `f64::clamp` preserves NaN, so `p` maps NaN to `P_FLOOR`.
    // An infinite pressure gives `h == 0.0`, and `0.0 / 0.0` matches no tier boundary.
    let p = if budget_pressure.is_nan() {
        P_FLOOR
    } else {
        budget_pressure.clamp(P_FLOOR, f64::MAX)
    };
    let f = 2f64.powf((imp - 50.0) / D);
    let h = (H50 * f) / p;
    a / h
}

/// Maps a decay position onto the fixed tier ladder.
///
/// Boundaries are lower-inclusive for the older tier: `z == Z4` yields P5.
#[inline]
fn tier_for_z(z: f64) -> Tier {
    if z < Z1 {
        Tier::P1
    } else if z < Z2 {
        Tier::P2
    } else if z < Z3 {
        Tier::P3
    } else if z < Z4 {
        Tier::P4
    } else {
        Tier::P5
    }
}

/// Tests a decay position against the anchor-adjusted archive boundary.
///
/// Anchor overlap clamps to 0 through 1 and raises the boundary by up to
/// [`G`] half-lives, so `archives_at(z, 0.0) == (tier_for_z(z) == Tier::P5)`.
/// `f64::clamp` preserves NaN, so a NaN overlap maps to 0 first.
#[inline]
fn archives_at(z: f64, anchor_overlap: f64) -> bool {
    let o = if anchor_overlap.is_nan() {
        0.0
    } else {
        anchor_overlap.clamp(0.0, 1.0)
    };
    z >= Z4 + G * o
}

/// Maps compartment age onto fixed exponential-decay boundaries.
///
/// `compartment_index` is one-based from newest and clamps upward to 1.
/// `importance` clamps to 1 through 100. `budget_pressure` has a floor of
/// [`P_FLOOR`] and a ceiling of `f64::MAX`; NaN reads as [`P_FLOOR`].
///
/// Boundaries are lower-inclusive for the older tier: a value
/// exactly equal to [`Z1`], [`Z2`], [`Z3`], or [`Z4`] enters the next tier.
pub fn tier(compartment_index: u32, importance: i32, budget_pressure: f64) -> Tier {
    tier_for_z(z_value(compartment_index, importance, budget_pressure))
}

/// Tests decay position against an anchor-adjusted archive boundary.
///
/// P5 denotes archival and is not rendered. Anchor overlap clamps to 0 through
/// 1 and raises the archive threshold by up to [`G`] half-lives. With
/// `anchor_overlap = 0.0`, archiving requires `z >= Z4`.
pub fn should_archive(
    compartment_index: u32,
    importance: i32,
    budget_pressure: f64,
    anchor_overlap: f64,
) -> bool {
    archives_at(
        z_value(compartment_index, importance, budget_pressure),
        anchor_overlap,
    )
}

/// Applies archival protection before selecting a renderable tier.
///
/// P5 denotes archival, not a verbosity tier. A compartment naturally in P5
/// remains P4 while anchor overlap protects it from archival.
pub fn rendered_tier(
    compartment_index: u32,
    importance: i32,
    budget_pressure: f64,
    anchor_overlap: f64,
) -> Tier {
    let z = z_value(compartment_index, importance, budget_pressure);
    if archives_at(z, anchor_overlap) {
        return Tier::P5;
    }
    match tier_for_z(z) {
        Tier::P5 => Tier::P4,
        t => t,
    }
}

/// Derives pressure from natural tier costs and a history budget.
///
/// `importances` is ordered newest first; position `i` is compartment index
/// `i + 1`. `history_budget` and tier costs use estimated tokens. A
/// non-positive budget returns 1. Otherwise, `H ∝ 1/p` makes per-tier
/// compartment counts scale as `1/p`, so `C(p) ≈ C(1)/p`; `p = C(1)/B`
/// targets budget `B` before applying [`P_FLOOR`]. Archived P5 compartments
/// contribute no natural cost.
pub fn compute_budget_pressure(importances: &[i32], history_budget: f64) -> f64 {
    if history_budget <= 0.0 {
        return 1.0;
    }
    let mut natural_cost = 0.0;
    for (i, &importance) in importances.iter().enumerate() {
        let natural_tier = tier((i + 1) as u32, importance, 1.0);
        if natural_tier != Tier::P5 {
            natural_cost += natural_tier.cost() as f64;
        }
    }
    (natural_cost / history_budget).max(P_FLOOR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[test]
    fn age_monotonic_demotion() {
        // for fixed importance/pressure, tier is non-decreasing in age.
        let mut prev = Tier::P1;
        for idx in 1..=400u32 {
            let t = tier(idx, 50, 1.0);
            assert!(t >= prev, "tier decreased with age at idx {idx}");
            prev = t;
        }
    }

    #[test]
    fn importance_protects_against_demotion() {
        // higher importance → same-or-lower tier (more protection) at fixed age/pressure.
        for idx in [10u32, 50, 200] {
            let lo = tier(idx, 10, 1.0);
            let hi = tier(idx, 90, 1.0);
            assert!(hi <= lo, "higher importance demoted faster at idx {idx}");
        }
    }

    #[test]
    fn pressure_accelerates_demotion() {
        for idx in [10u32, 50, 200] {
            let lo = tier(idx, 50, 0.5);
            let hi = tier(idx, 50, 4.0);
            assert!(hi >= lo, "higher pressure protected more at idx {idx}");
        }
    }

    #[test]
    fn non_finite_pressure_keeps_the_newest_compartment_in_tier_1() {
        assert_eq!(tier(1, 50, f64::INFINITY), Tier::P1);
        assert_eq!(tier(1, 50, f64::MAX), Tier::P1);
        assert_eq!(tier(2, 50, f64::INFINITY), Tier::P5);
        assert_eq!(tier(2, 50, f64::MAX), Tier::P5);
        for idx in [1u32, 2, 50] {
            assert_eq!(tier(idx, 50, f64::NAN), tier(idx, 50, P_FLOOR));
        }
    }

    #[test]
    fn nan_anchor_overlap_reads_as_zero() {
        let (idx, imp, p) = (BOUNDARY_INDEX, 50, 3.0);
        assert!(should_archive(idx, imp, p, f64::NAN));
        assert_eq!(rendered_tier(idx, imp, p, f64::NAN), Tier::P5);
        assert!(should_archive(idx, imp, p, f64::NEG_INFINITY));
        assert!(!should_archive(idx, imp, p, f64::INFINITY));
    }

    /// Importance 50 at index 25 has `a = 24 = H50`, so `z == pressure` exactly.
    const BOUNDARY_INDEX: u32 = 25;

    #[test]
    fn tier_boundaries_are_lower_inclusive_for_older_tier() {
        for (boundary, newer, older) in [
            (Z1, Tier::P1, Tier::P2),
            (Z2, Tier::P2, Tier::P3),
            (Z3, Tier::P3, Tier::P4),
            (Z4, Tier::P4, Tier::P5),
        ] {
            assert_eq!(
                tier(BOUNDARY_INDEX, 50, boundary),
                older,
                "z == {boundary} must enter the older tier"
            );
            assert_eq!(
                tier(BOUNDARY_INDEX, 50, boundary.next_down()),
                newer,
                "z one ULP below {boundary} must stay in the newer tier"
            );
        }
    }

    #[test]
    fn archive_boundary_is_inclusive_at_z4() {
        assert!(should_archive(BOUNDARY_INDEX, 50, Z4, 0.0));
        assert!(!should_archive(BOUNDARY_INDEX, 50, Z4.next_down(), 0.0));
        assert_eq!(rendered_tier(BOUNDARY_INDEX, 50, Z4, 0.0), Tier::P5);
        assert_eq!(
            rendered_tier(BOUNDARY_INDEX, 50, Z4.next_down(), 0.0),
            Tier::P4
        );
    }

    #[test]
    fn anchor_overlap_extends_archive_boundary_and_clamps() {
        // z = 3.0 satisfies Z4 <= z < Z4 + G: natural P5, inside the G-wide anchor window.
        let (idx, imp, p) = (BOUNDARY_INDEX, 50, 3.0);
        assert_eq!(tier(idx, imp, p), Tier::P5);
        assert!(should_archive(idx, imp, p, 0.0));
        assert!(!should_archive(idx, imp, p, 1.0));
        // Anchor protection caps the rendered tier at P4 instead of archiving.
        assert_eq!(rendered_tier(idx, imp, p, 1.0), Tier::P4);
        assert_eq!(
            should_archive(idx, imp, p, 5.0),
            should_archive(idx, imp, p, 1.0)
        );
        assert_eq!(
            should_archive(idx, imp, p, -1.0),
            should_archive(idx, imp, p, 0.0)
        );
    }

    #[test]
    fn non_positive_budget_yields_unit_pressure() {
        assert_eq!(compute_budget_pressure(&[50], 0.0), 1.0);
        assert_eq!(compute_budget_pressure(&[50], -1.0), 1.0);
    }

    #[test]
    fn pressure_self_tunes_toward_budget() {
        let importances = [50; 200];
        // For nonzero natural cost and `history_budget > 0`, pressure increases as the budget decreases, subject to [`P_FLOOR`].
        let loose = compute_budget_pressure(&importances, 20_000.0);
        let tight = compute_budget_pressure(&importances, 2_000.0);
        assert!(tight > loose);
        assert!(loose >= P_FLOOR);
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TierCase {
        index: u32,
        importance: i32,
        pressure: f64,
        tier: u8,
        archived: bool,
        rendered: u8,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PressureCase {
        importances: Vec<i32>,
        budget: f64,
        one_pass: f64,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Golden {
        tier_cases: Vec<TierCase>,
        pressure_cases: Vec<PressureCase>,
    }

    /// Maps the fixture's numeric tier column onto [`Tier`].
    fn tier_from_golden(tier: u8) -> Tier {
        match tier {
            1 => Tier::P1,
            2 => Tier::P2,
            3 => Tier::P3,
            4 => Tier::P4,
            5 => Tier::P5,
            other => panic!("golden tier {other} out of range 1..=5"),
        }
    }

    #[test]
    fn decay_golden_matches_reference() {
        let raw = include_str!("../testdata/decay-golden.json");
        let golden: Golden = serde_json::from_str(raw).expect("parse decay-golden.json");
        assert!(!golden.tier_cases.is_empty(), "empty tier grid");

        for c in &golden.tier_cases {
            assert_eq!(
                tier(c.index, c.importance, c.pressure),
                tier_from_golden(c.tier),
                "tier mismatch at idx={} imp={} p={}",
                c.index,
                c.importance,
                c.pressure
            );
            assert_eq!(
                should_archive(c.index, c.importance, c.pressure, 0.0),
                c.archived,
                "archive mismatch at idx={} imp={} p={}",
                c.index,
                c.importance,
                c.pressure
            );
            assert_eq!(
                rendered_tier(c.index, c.importance, c.pressure, 0.0),
                tier_from_golden(c.rendered),
                "rendered mismatch at idx={} imp={} p={}",
                c.index,
                c.importance,
                c.pressure
            );
        }

        for c in &golden.pressure_cases {
            let one = compute_budget_pressure(&c.importances, c.budget);
            assert!(
                (one - c.one_pass).abs() < 1e-9,
                "one-pass pressure mismatch: rust={one} ts={} budget={}",
                c.one_pass,
                c.budget
            );
        }
    }
}

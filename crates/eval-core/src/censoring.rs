//! Right-censored outcomes: latency percentiles that keep every timed-out or
//! budget-exhausted attempt in the denominator, zero-failure counts reported as
//! an upper bound rather than a proof, and repeated live trials summarized as
//! pass@1 and a pass^k interval. Every fraction is an exact [`Ratio`].

use serde::{Deserialize, Serialize};

use crate::statistics::{ArmResult, CensorReason, ClusteringUnit, Ratio, StatisticsError, gcd};

/// The smallest sample a p99 may be quoted from: the third-largest of 299
/// observations sits at the 99th percentile rank.
pub const P99_MIN_RUNS: usize = 299;

/// One measured attempt. A censored attempt's `duration_ms` is its censoring
/// point, the elapsed time at which the run was cut off, and its true duration
/// is at least that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub duration_ms: u64,
    pub censored: Option<CensorReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PercentileBound {
    Point,
    /// A censored attempt sorts at or below the rank, so the true order
    /// statistic is at least the reported value.
    Lower,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Percentile {
    pub p: u8,
    pub value: u64,
    pub n: u32,
    pub censored: u32,
    pub bound: PercentileBound,
}

/// Censored attempts stay in the denominator, ordered by their censoring point
/// and after a completed attempt of equal duration. Raising a censored value
/// can only raise an order statistic, so a percentile is a point only when at
/// least `rank` completed attempts sit at or below the picked value; otherwise
/// it is a lower bound.
/// `p99` needs [`P99_MIN_RUNS`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LatencySummary {
    pub n: u32,
    pub censored: u32,
    pub percentiles: Vec<Percentile>,
}

impl LatencySummary {
    pub fn of(attempts: &[Attempt]) -> Self {
        let mut sorted: Vec<&Attempt> = attempts.iter().collect();
        sorted.sort_by_key(|attempt| (attempt.duration_ms, attempt.censored.is_some()));
        let n = sorted.len();
        let censored = sorted.iter().filter(|a| a.censored.is_some()).count();
        let percentile = |p: u8| {
            // Nearest rank: the smallest rank r with r/n >= p/100.
            let rank = (usize::from(p) * n).div_ceil(100);
            let value = sorted[rank - 1].duration_ms;
            // With every censored attempt pushed to infinity the order statistic is the
            // rank-th completed duration, which is still `value` exactly when at least
            // `rank` completions sit at or below it.
            let settled = sorted
                .iter()
                .filter(|a| a.censored.is_none() && a.duration_ms <= value)
                .count();
            Percentile {
                p,
                value,
                n: n as u32,
                censored: censored as u32,
                bound: if settled >= rank {
                    PercentileBound::Point
                } else {
                    PercentileBound::Lower
                },
            }
        };
        let mut percentiles = Vec::new();
        if n > 0 {
            percentiles.push(percentile(50));
            percentiles.push(percentile(95));
            if n >= P99_MIN_RUNS {
                percentiles.push(percentile(99));
            }
        }
        Self {
            n: n as u32,
            censored: censored as u32,
            percentiles,
        }
    }
}

/// A count of failures over `n` trials at the named cluster unit. Zero
/// failures is an upper bound, never a proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counter {
    pub n: u64,
    pub failures: u64,
    pub unit: ClusteringUnit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "evidence_kind", deny_unknown_fields)]
pub enum FailureRate {
    Observed {
        rate: Ratio,
        upper_bound_95: Ratio,
        bound_method: BoundMethod,
        n: u64,
        unit: ClusteringUnit,
    },
    /// The rule of three: `3/n` (at most one) bounds the rate at 95 percent
    /// when nothing failed in `n` trials.
    Bound {
        upper_bound_95: Ratio,
        bound_method: BoundMethod,
        n: u64,
        unit: ClusteringUnit,
    },
}

impl FailureRate {
    /// The one number a gate compares against its threshold.
    pub fn upper_bound_95(&self) -> Ratio {
        match self {
            Self::Observed { upper_bound_95, .. } | Self::Bound { upper_bound_95, .. } => {
                *upper_bound_95
            }
        }
    }
}

/// `RuleOfThree` uses `3/n` when `failures` is zero; `PoissonEnvelope` uses
/// `(2 * failures + 3) / n` otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundMethod {
    RuleOfThree,
    PoissonEnvelope,
}

impl Counter {
    pub fn rate(&self) -> Result<FailureRate, StatisticsError> {
        if self.n == 0 || self.failures > self.n {
            return Err(StatisticsError::MalformedCounter {
                n: self.n,
                failures: self.failures,
            });
        }
        let n = i128::from(self.n);
        let failures = i128::from(self.failures);
        let upper_bound_95 = Ratio::try_new(2 * failures + 3, n)?.min(Ratio::ONE);
        Ok(if self.failures == 0 {
            FailureRate::Bound {
                upper_bound_95,
                bound_method: BoundMethod::RuleOfThree,
                n: self.n,
                unit: self.unit,
            }
        } else {
            FailureRate::Observed {
                rate: Ratio::try_new(failures, n)?,
                upper_bound_95,
                bound_method: BoundMethod::PoissonEnvelope,
                n: self.n,
                unit: self.unit,
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum PassKBounds {
    /// Two censoring conventions, not one confidence interval: `censored_as_fail`
    /// counts every censored attempt as a failure over all repeats, and
    /// `censored_excluded` counts only the uncensored attempts (one when fewer
    /// than `k` remain, which `uncensored_repeats` shows).
    Bounds {
        censored_as_fail: Ratio,
        censored_excluded: Ratio,
    },
    /// Every attempt was censored; the evidence says nothing.
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassK {
    pub k: u32,
    pub repeats: u32,
    pub uncensored_repeats: u32,
    /// Passes over all repeats; a censored attempt is not a pass.
    pub pass_at_1: Ratio,
    pub censoring_rate: Ratio,
    pub pass_k: PassKBounds,
}

fn choose(n: u64, k: u64) -> Result<u128, StatisticsError> {
    if k > n {
        return Ok(0);
    }
    // C(n, k) = C(n, n - k); the shorter walk stays clear of the central coefficients.
    let k = k.min(n - k);
    // C(n, i + 1) = C(n, i) * (n - i) / (i + 1). Dividing before multiplying keeps every
    // intermediate at most the coefficient it produces: after g = gcd(C(n, i), i + 1) leaves
    // the accumulator, the rest of i + 1 is coprime to it and so divides n - i.
    (0..k).try_fold(1u128, |acc, i| {
        let g = gcd(acc, u128::from(i + 1));
        (acc / g)
            .checked_mul(u128::from(n - i) / (u128::from(i + 1) / g))
            .ok_or(StatisticsError::RationalOverflow)
    })
}

/// `C(passes, k) / C(n, k)`: the probability that `k` draws without
/// replacement from the `n` attempts are all passes. Fewer than `k` attempts
/// bound nothing, so the value is one.
fn pass_power_k(passes: u64, n: u64, k: u64) -> Result<Ratio, StatisticsError> {
    let (numerator, denominator) = (choose(passes, k)?, choose(n, k)?);
    if denominator == 0 {
        return Ok(Ratio::ONE);
    }
    // Reduce while still in u128: the coefficients may sit above i128::MAX when the ratio
    // itself is small.
    let divisor = gcd(numerator, denominator);
    let (numerator, denominator) = (numerator / divisor, denominator / divisor);
    let fits = |value: u128| i128::try_from(value).map_err(|_| StatisticsError::RationalOverflow);
    Ratio::try_new(fits(numerator)?, fits(denominator)?)
}

/// Summarizes repeated live trials of one task under the frozen repeat count
/// `k`. A `k` of zero, no attempts, or `k` past the repeat count refuses.
pub fn pass_k(attempts: &[ArmResult], k: u32) -> Result<PassK, StatisticsError> {
    let repeats = attempts.len() as u64;
    if k == 0 || repeats == 0 || u64::from(k) > repeats {
        return Err(StatisticsError::MalformedTrials {
            k,
            repeats: repeats as u32,
        });
    }
    let passes = attempts.iter().filter(|a| **a == ArmResult::Pass).count() as u64;
    let censored = attempts
        .iter()
        .filter(|a| matches!(a, ArmResult::Censored(_)))
        .count() as u64;
    let pass_k = if censored == repeats {
        PassKBounds::Indeterminate
    } else {
        PassKBounds::Bounds {
            censored_as_fail: pass_power_k(passes, repeats, u64::from(k))?,
            censored_excluded: pass_power_k(passes, repeats - censored, u64::from(k))?,
        }
    };
    Ok(PassK {
        k,
        repeats: repeats as u32,
        uncensored_repeats: (repeats - censored) as u32,
        pass_at_1: Ratio::try_new(i128::from(passes), i128::from(repeats))?,
        censoring_rate: Ratio::try_new(i128::from(censored), i128::from(repeats))?,
        pass_k,
    })
}

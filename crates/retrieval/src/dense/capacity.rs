//! The size of the quantized candidate pool: `R = ceil(alpha * K)`, computed exactly from alpha's binary value and checked against an approved cap before anything R-sized is allocated.
//! Alpha is checked first, so a malformed alpha refuses even when `K` is zero; a zero `K` with a valid alpha yields no capacity, and a caller that holds no capacity starts no embedding and no scan.

use std::num::NonZeroUsize;

/// How the pool size follows from `K`; the cap is the approved maximum pool, not a default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CandidatePolicy {
    /// Finite and at least one, so the pool is never smaller than the ranking it feeds.
    pub alpha: f64,
    pub cap: NonZeroUsize,
}

#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum CapacityRefusal {
    #[error("alpha {alpha} is not a finite number of at least one")]
    Alpha { alpha: f64 },
    /// `ceil(alpha * K)` does not fit a `usize`.
    #[error("ceil({alpha} * {k}) is not representable")]
    Unrepresentable { alpha: f64, k: usize },
    #[error("a pool of {candidates} candidates exceeds the approved cap of {cap}")]
    OverCap { candidates: usize, cap: usize },
}

/// The checked ranking size `K` and pool size `R`, with `K <= R <= cap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateCapacity {
    k: NonZeroUsize,
    candidates: NonZeroUsize,
}

impl CandidateCapacity {
    /// # Errors
    ///
    /// In this order: an alpha that is not finite or is below one, a pool size that does not fit a `usize`, and a pool larger than the cap.
    /// `Ok(None)` is a zero `k` under a valid alpha: an empty ranking that needs no work.
    pub fn new(k: usize, policy: CandidatePolicy) -> Result<Option<Self>, CapacityRefusal> {
        let alpha = policy.alpha;
        if !(alpha.is_finite() && alpha >= 1.0) {
            return Err(CapacityRefusal::Alpha { alpha });
        }
        let Some(k) = NonZeroUsize::new(k) else {
            return Ok(None);
        };
        let candidates = ceil_product(alpha, k.get())
            .and_then(NonZeroUsize::new)
            .ok_or(CapacityRefusal::Unrepresentable { alpha, k: k.get() })?;
        if candidates > policy.cap {
            return Err(CapacityRefusal::OverCap {
                candidates: candidates.get(),
                cap: policy.cap.get(),
            });
        }
        Ok(Some(Self { k, candidates }))
    }

    /// The ranking size the rescore returns at most.
    pub fn k(&self) -> NonZeroUsize {
        self.k
    }

    /// The pool size the quantized scan accepts at most.
    pub fn candidates(&self) -> NonZeroUsize {
        self.candidates
    }
}

/// `ceil(alpha * k)` in exact integer arithmetic: a finite alpha of at least one is `m * 2^e` with a 53-bit `m`, so `m * k` fits a `u128` and the scaling by `2^e` is a shift.
fn ceil_product(alpha: f64, k: usize) -> Option<usize> {
    let bits = alpha.to_bits();
    let biased = i32::try_from((bits >> 52) & 0x7ff).expect("eleven bits fit an i32");
    let mantissa = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    // An alpha of at least one is normal, so its value is `mantissa * 2^(biased - 1075)`.
    let exponent = biased - 1075;
    let product = u128::from(mantissa) * k as u128;
    let scaled = if exponent >= 0 {
        let shift = u32::try_from(exponent).expect("a nonnegative exponent fits a u32");
        if shift >= 128 || product > (u128::MAX >> shift) {
            return None;
        }
        product << shift
    } else {
        let shift = exponent.unsigned_abs();
        let unit = 1u128 << shift;
        product.div_ceil(unit)
    };
    usize::try_from(scaled).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exact_ceiling_matches_small_cases_and_powers_of_two() {
        assert_eq!(ceil_product(1.0, 64), Some(64));
        assert_eq!(ceil_product(4.0, 64), Some(256));
        assert_eq!(ceil_product(1.5, 3), Some(5));
        assert_eq!(ceil_product(2.5, 2), Some(5));
        assert_eq!(ceil_product(1.0, usize::MAX), Some(usize::MAX));
        assert_eq!(ceil_product(2.0, usize::MAX), None);
        assert_eq!(ceil_product(f64::MAX, 1), None);
    }

    /// `1.1` is slightly above eleven tenths in binary, so `1.1 * 10` rounds to exactly `11.0` in f64 while the exact product exceeds eleven.
    #[test]
    fn the_ceiling_is_taken_of_the_exact_product_not_its_f64_rounding() {
        let alpha = 1.1f64;
        assert_eq!(alpha * 10.0, 11.0, "the f64 product rounds to eleven");
        assert_eq!(ceil_product(alpha, 10), Some(12));
    }
}

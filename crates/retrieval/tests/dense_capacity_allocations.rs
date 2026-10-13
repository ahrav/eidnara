//! Allocator evidence for the candidate capacity: every refusal and a zero `K` allocate nothing, and the pool a checked capacity sizes is one allocation proportional to `R`.

#[path = "../../daemon/tests/support/alloc_recorder.rs"]
#[allow(dead_code)]
mod alloc_recorder;

#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

use std::num::NonZeroUsize;

use retrieval::dense::score::TopK;
use retrieval::dense::{CandidateCapacity, CandidatePolicy, CapacityRefusal};

fn policy(alpha: f64, cap: usize) -> CandidatePolicy {
    CandidatePolicy {
        alpha,
        cap: NonZeroUsize::new(cap).unwrap(),
    }
}

#[test]
fn every_capacity_refusal_and_a_zero_k_allocate_nothing() {
    let cases: [(
        usize,
        CandidatePolicy,
        Result<Option<usize>, CapacityRefusal>,
    ); 6] = [
        (
            64,
            policy(f64::NAN, 256),
            Err(CapacityRefusal::Alpha { alpha: f64::NAN }),
        ),
        (
            0,
            policy(0.5, 256),
            Err(CapacityRefusal::Alpha { alpha: 0.5 }),
        ),
        (
            usize::MAX,
            policy(2.0, usize::MAX),
            Err(CapacityRefusal::Unrepresentable {
                alpha: 2.0,
                k: usize::MAX,
            }),
        ),
        (
            65,
            policy(4.0, 256),
            Err(CapacityRefusal::OverCap {
                candidates: 260,
                cap: 256,
            }),
        ),
        (0, policy(4.0, 256), Ok(None)),
        (64, policy(4.0, 256), Ok(Some(256))),
    ];
    for (k, policy, expected) in cases {
        let (result, ledger) = alloc_recorder::record_window(|| CandidateCapacity::new(k, policy));
        let result = result.map(|capacity| capacity.map(|capacity| capacity.candidates().get()));
        // `NaN` never equals itself, so the alpha refusal compares its bits.
        match (&result, &expected) {
            (
                Err(CapacityRefusal::Alpha { alpha: got }),
                Err(CapacityRefusal::Alpha { alpha: want }),
            ) => assert_eq!(got.to_bits(), want.to_bits()),
            _ => assert_eq!(result, expected, "k {k}"),
        }
        assert_eq!(
            (ledger.allocation_events, ledger.requested_bytes),
            (0, 0),
            "k {k} {policy:?}: the capacity check allocates nothing"
        );
    }
}

/// A pool exists only for a checked capacity, so a refused capacity, which yields no `R`, allocates no pool.
#[test]
fn the_pool_a_capacity_sizes_is_one_allocation_proportional_to_r() {
    let pool_bytes = |k: usize, alpha: f64| {
        let capacity = CandidateCapacity::new(k, policy(alpha, 1 << 20))
            .unwrap()
            .unwrap();
        let (pool, ledger) =
            alloc_recorder::record_window(|| TopK::<()>::new(capacity.candidates()));
        drop(pool);
        assert_eq!(ledger.allocation_events, 1, "one heap allocation");
        ledger.requested_bytes
    };
    let small = pool_bytes(4, 1.0);
    assert!(small > 0);
    assert_eq!(pool_bytes(64, 4.0), small * 64, "R = 256 is 64 times R = 4");
    assert_eq!(
        pool_bytes(10, 1.1),
        small * 3,
        "the f64 nearest 1.1 exceeds 1.1, so the exact ceiling of its product with 10 is 12"
    );
}

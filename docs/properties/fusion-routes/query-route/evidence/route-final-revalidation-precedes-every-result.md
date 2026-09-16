# route-final-revalidation-precedes-every-result

## Discovery trigger

The RP2.7 specification requires final canonical revalidation through the
budget-aware kernel eligibility adapter on every result, with no
retrieval-side verdict copy.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u2-weighted-rrf` at
`6dea07f455d536eec50556116c882c8dc6a00d98`; inspected 2026-09-16.

## Evidence trail

- `crates/retrieval/src/eligibility.rs` `live_candidates_by_id` reads the
  fused occurrences' candidate tuples; `judge_occurrences_within_budget`
  judges them against the kernel.
- `crates/daemon/src/query_route.rs` `execute` filters the fused set with
  `Fused::filter`, which keeps positions and scores.
- `crates/daemon/tests/query_route.rs`
  `revalidation_excludes_retired_and_foreign_occurrences`.

## Failure scenario

A retired or corrected occurrence is returned with a live position.

## Timing windows and dependencies

The retirement lands between the projection snapshot and the query.

## What a test must construct

- A projection built from a kernel snapshot.
- A `retire_decision` commit and a `ClaimMaterializer` episode so the kernel's
  descriptors carry the retirement.

## Investigation log

### Q: Why is the candidate read inside the projection transaction?

- Sources examined: `execute`'s `read_under` closure.
- Findings: reading candidates in the same read as the lanes keeps them at one
  projection snapshot; the kernel judgement runs after the read so the
  projection connection is not held during kernel I/O.
- Missing evidence: none.
- Conclusion: resolved with answer.

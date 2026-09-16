# route-final-revalidation-precedes-every-result

## Discovery trigger

The RP2.7 specification requires final canonical revalidation through the
budget-aware kernel eligibility adapter on every result, with no
retrieval-side verdict copy.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u2-weighted-rrf` at
`6dea07f455d536eec50556116c882c8dc6a00d98`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/query_route.rs` `exact_read` builds an
  `OccurrenceCandidate` from each `AssociationRow`, and
  `crates/retrieval/src/lexical/retrieve.rs` `Contribution` carries the
  `EligibilityCandidate` the kernel admitted it with; `admit_exact` and
  `admit_lexical` keep the admitted candidates keyed by `OccurrenceId`, so
  revalidation needs no second projection read.
- `crates/retrieval/src/eligibility.rs` `judge_tracked` judges one slice and
  `authority_moved` compares its snapshot and incarnation with the first
  slice's; `crates/daemon/src/query_route.rs` `judge_eligible` stops on the
  first move.
- `crates/daemon/src/query_route.rs` `execute` filters the fused set with
  `Fused::filter`, which keeps positions and scores.
- `crates/daemon/tests/query_route.rs`
  `revalidation_excludes_retired_and_foreign_occurrences` and
  `revalidation_refuses_verdicts_joined_across_a_moved_kernel_snapshot`.

## Failure scenario

A retired or corrected occurrence is returned with a live position, or an
answer joins verdicts taken before and after a canonical change.

## Timing windows and dependencies

The retirement lands between the projection snapshot and the query; a commit
lands between two revalidation slices.

## What a test must construct

- A projection built from a kernel snapshot.
- A `retire_decision` commit and a `ClaimMaterializer` episode so the kernel's
  descriptors carry the retirement.
- A `validation_batch` of one and a kernel commit placed by the `before_phase`
  hook at the second revalidation check.

## Investigation log

### Q: Where do the candidate terms revalidation judges come from?

- Sources examined: `execute`'s `read_under` closure; `exact::AssociationRow`;
  `lexical::Contribution`.
- Findings: both lanes already read every column the kernel's
  `EligibilityCandidate` needs, so each lane hands its admitted candidates to
  revalidation and the projection connection is held only for the lanes' own
  statements. Every kernel reader, for admission and for revalidation, is taken
  after the connection is released.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Why is a moved kernel a refusal rather than a degraded answer?

- Sources examined: `crates/retrieval/src/eligibility.rs` `judge_occurrences`
  ("the caller pages rather than joining verdicts from two snapshots") and
  `EgressSnapshot::classification_generation`; `lexical::admit`, which marks a
  moved slice `SnapshotChanged` and accepts none of it.
- Findings: a verdict taken after a commit describes other facts than one taken
  before it, and an unknown classification generation means a classification
  merge overlapped the read; the lexical lane already refuses both. The
  revalidation gate is the last authorization step, so it fails closed the same
  way.
- Missing evidence: none.
- Conclusion: resolved with answer - `lane_unavailable` with reason
  `snapshot_changed` or `kernel_incarnation_changed`.

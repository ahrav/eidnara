# route-dense-unavailable-degrades-typed-within-deadline

## Discovery trigger

The RP2.7 specification requires that a saturated or unavailable dense lane
returns an explicitly typed exact plus lexical result within the original
deadline, not `deadline` and not dense completion, and that validation
failure, cancellation, and corruption are never relabeled as degraded success
(parent Q5).

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u3b-query-route` at
`41fb5258`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/query_route.rs`: `EmbedFailure::Unavailable` becomes
  `DenseLane::Unavailable(reason)` and the lane status `unavailable`;
  `EmbedFailure::Faulted` is `lane_unavailable`/`embedding_failed`;
  `DenseRefusal::Corruption` is `lane_unavailable`/`dense_corruption`;
  `DenseRefusal::QueryShape` degrades the lane; `DenseRefusal::Budget` is the
  budget's verdict.
- `crates/daemon/tests/query_route_dense.rs`
  `an_unavailable_embedding_lane_degrades_to_a_nonempty_exact_and_lexical_answer`
  `producer_corruption_is_typed_while_a_foreign_query_shape_degrades_the_lane`,
  and `a_coverage_shortfall_and_a_row_bound_leave_the_dense_lane_incomplete`.

## Failure scenario

A busy lane is reported as the caller's deadline, or a corrupt vector is
ranked and served as a degraded success.

## Timing windows and dependencies

None.

## What a test must construct

- A projection with stored vectors; `DenseLane::Unavailable` per reason.
- A zero-norm stored vector encoded at the generation's dimension.

## Investigation log

### Q: Should a busy lane retry inside the deadline?

- Sources examined: `embed_blocking`, which refuses `Busy` without waiting.
- Findings: a retry would spend the caller's budget on the lane's queue; the
  degraded answer is available at once.
- Missing evidence: RP2.9 calibration.
- Conclusion: resolved with answer for this build - degrade at once; retry is
  an open calibration question.

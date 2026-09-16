# fusion-fuses-once-before-revalidation

## Discovery trigger

RP2.7 requires fusion to run once with revalidation filtering the fused set
without recomputing scores or positions.

## Evidence trail

- `Fused` exposes no lane rankings and no scoring entry point, so a consumer
  holding a fused ranking cannot rescore it.
- `Fused::filter` retains entries and returns the same `Fused` with survivors
  untouched.

## Failure scenario

A route that re-fused after filtering would change positions with every
eligibility decision.

## Timing windows and dependencies

None. Fusion is a pure function over values.

## What a test must construct

- A fused ranking with one entry filtered out, compared triple by triple.

## Investigation log

### Q: None.

- Sources examined: none needed.
- Findings: none.
- Missing evidence: none.
- Conclusion: resolved with answer - no open question.

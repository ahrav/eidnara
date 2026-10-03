# dense-pool-never-mixes-authority-states

## Discovery trigger

RP2.6 authority and progress, #610 AC4: a complete multibatch query must not
mix authority stamps, including decisions that excluded possible winners.

## Evidence trail

- `judge_tracked` (`crates/retrieval/src/eligibility.rs:282`) records the
  first batch's snapshot and incarnation and reports a later batch that
  differs.
- The walk stops at a moved authority and re-judges the held set; the
  candidate scan then clears the pool (`candidates.rs`).
- A kernel error from judgment propagates as a refusal.

## Failure scenario

A pool admitted under one snapshot and re-judged under another holds rows
whose exclusions no longer describe the facts.

## Timing windows and dependencies

Between batches and before the final re-judgment.

## What a test must construct

- A retirement, an admission of an excluded row, and a kernel restore inside
  the walk's hook; a corrupt identity field.

## Investigation log

### Q: Is a restart required?

- Sources examined: #610 scope.
- Findings: a typed non-result is sufficient; a restart needs owner approval.
- Missing evidence: none.
- Conclusion: resolved with answer - discard, no restart.

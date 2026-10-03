# dense-missing-original-quarantines-without-substitute

## Discovery trigger

RP2.6 failure rules and #613 AC4: missing accepted payload fails the query and
quarantines its generation; I/O failure and cancellation keep distinct
classifications.

## Evidence trail

- `PinnedLayer::original` classifies a short read and an index past the rows
  as `Missing`, codec refusals as `Rejected`, and other read errors as `Io`.
- `rank_compressed` sets the view's quarantine flag for `Missing`, `Rejected`,
  and code corruption found by the scan, and refuses `Quarantined` on every
  later call; `rank` refuses the same way.
- The rescore checks the budget before every read and refuses `Budget`
  without touching the flag.
- `vector_composition::recover` and `acquire` re-verify every member, so a
  member whose files no longer hash is skipped for the prior verified
  composition.

## Failure scenario

Serving a shorter ranking or an older row hides the corruption and returns
scores the generation never held.

## Timing windows and dependencies

Between selection and the reads.

## What a test must construct

- A rows file cut to its header, NaN rows, a cancelled budget, a prior
  composition, and a retirement in the kernel.

## Investigation log

### Q: Is the quarantine persistent?

- Sources examined: #578 Q4, `GenerationStore`, `vector_composition`.
- Findings: the store has no corruption marker; re-verification on
  acquisition refuses a member whose bytes changed.
- Missing evidence: owner decision on a durable marker.
- Conclusion: unresolved, needs Q4 owner decision.

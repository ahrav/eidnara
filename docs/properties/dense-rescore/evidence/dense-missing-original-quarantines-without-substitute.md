# dense-missing-original-quarantines-without-substitute

## Discovery trigger

RP2.6 failure rules and #613 AC4: missing accepted payload fails the query and
quarantines its generation; I/O failure and cancellation keep distinct
classifications.

## Evidence trail

- `PinnedLayer::read_row_bytes` classifies an index past the rows and a short
  read as `RowFault::Missing` and any other read error as
  `RowFault::Unavailable` (`read_fault`); `RowAccess::row` adds codec
  refusals as `Rejected`.
- `layered::fault_refusal` maps `Missing` to `OracleRefusal::Unreadable` and
  `Unavailable` to `OracleRefusal::ReadFailed`, so the scan keeps the two
  apart too.
- `rescore_refusal` quarantines the view for `Missing` and `Rejected`, and
  `rank_compressed` for codes the scan finds unreadable or malformed; both
  refuse `Quarantined` on every later call, and so does `rank`.
- The rescore checks the budget before every read and refuses `Budget`
  without touching the flag.
- `PinnedVectors::fail_original_reads_for_test` (test-support only) hands an
  error kind to `rank_compressed` in place of the positioned read, so the
  production classifier sees an ordinary read error: the ranking refuses
  `Io` naming the member and the pool's best entry after one attempted read,
  the flag stays clear, and the same view ranks the same result once reads
  succeed. The route maps that refusal to `read_failed` for the dense lane
  alone and releases the charges. The same seam reporting `UnexpectedEof`
  refuses `Missing` and quarantines.
- `vector_composition::recover` and `acquire` re-verify every member, so a
  member whose files no longer hash is skipped for the prior verified
  composition. Re-verification reads only durable state, so a process that
  reopens the store after another process cut a member's rows and exited
  without destructors recovers the prior verified composition, reports the
  selector `Stale`, and never reads the cut member.
- With no prior verified composition, `recover` and `acquire` refuse
  `NoCompatibleTarget` until a rebuilt layer is published under a new
  composition; every later read names the rebuilt member.

## Failure scenario

Serving a shorter ranking or an older row hides the corruption and returns
scores the generation never held.

## Timing windows and dependencies

Between selection and the reads.

## What a test must construct

- A rows file cut to its header, NaN rows, a cancelled budget, a prior
  composition, and a retirement in the kernel.
- An accepted-row read that reports an error other than a short read, driven
  through `fail_original_reads_for_test`, followed by a read that succeeds on
  the same view; the route run under the same failure.
- A composition whose only member is corrupt with no earlier verified
  composition, then a rebuilt layer published under a new composition.
- A store left by a child process that published a composition and its
  replacement, cut the replacement's rows to the header, and exited without
  running destructors; the parent reopens that store with a fresh gate and
  ledger.

## Investigation log

### Q: Is the quarantine persistent?

- Sources examined: #578 Q4, `GenerationStore`, `vector_composition`.
- Findings: the store has no corruption marker; re-verification on
  acquisition refuses a member whose bytes changed.
- Missing evidence: owner decision on a durable marker.
- Conclusion: unresolved, needs Q4 owner decision.

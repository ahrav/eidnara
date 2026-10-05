# dense-pool-is-the-top-r-of-the-eligible-resolved-set

## Discovery trigger

RP2.6 R1 to R3 and #610 AC1 and AC3: a complete scan yields
`A = Top(R_dense, E, quantized_score)` over resolved winners only.

## Evidence trail

- `select_candidates` (`crates/retrieval/src/dense/candidates.rs:263`)
  encodes the query under every layer's scales, resolves the layers, and walks
  the live required rows in rowid order through `ResolvedCodes`.
- `ResolvedCodes::visit` finds each visited row's winner through
  `Cursor::find` (`crates/retrieval/src/dense/layered.rs:147`) and marks it
  live, so rows a newer layer superseded or masked have no winner, and only
  live winners are scored once the walk ends.
- The pool's capacity is `CandidateCapacity::candidates`
  (`candidates.rs:324`).

## Failure scenario

Scoring a superseded row, or a delta's codes under the base's scales, ranks
the pool by values the generation does not define.

## Timing windows and dependencies

None for the stable case.

## What a test must construct

- Unadmitted leaders, a delta with its own calibration that supersedes and
  masks, an underfilled pool.

## Investigation log

### Q: Does the pool depend on page size?

- Sources examined: the walk's admission rule.
- Findings: every live winner is scored before any is judged, and rows are
  judged best first across the whole walk; the test runs page sizes 1, 2, and
  8.
- Missing evidence: none.
- Conclusion: resolved with answer - no.

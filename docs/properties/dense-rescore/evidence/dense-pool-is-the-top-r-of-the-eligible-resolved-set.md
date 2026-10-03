# dense-pool-is-the-top-r-of-the-eligible-resolved-set

## Discovery trigger

RP2.6 R1 to R3 and #610 AC1 and AC3: a complete scan yields
`A = Top(R_dense, E, quantized_score)` over resolved winners only.

## Evidence trail

- `select_candidates` (`crates/retrieval/src/dense/candidates.rs:192`)
  encodes the query under every layer's scales, resolves the layers, and walks
  the live required rows through `ResolvedCodes`.
- `ResolvedCodes::load` draws each visited row's winner through the shared
  `Cursor::seek` (`crates/retrieval/src/dense/layered.rs:99`), so rows a newer
  layer superseded or masked are never loaded.
- The pool's capacity is `CandidateCapacity::candidates`
  (`candidates.rs:253`).

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
- Findings: a row enters judgment only when it could enter the pool, and the
  final pool is re-judged; the test runs page sizes 1, 2, and 8.
- Missing evidence: none.
- Conclusion: resolved with answer - no.

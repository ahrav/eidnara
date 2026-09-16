# fusion-one-contribution-per-lane

## Discovery trigger

RP2.7 R2 requires each lane to be deduplicated to one occurrence ranking
before fusion so probes and generations cannot multiply contribution.

## Evidence trail

- `crates/retrieval/src/fusion/lane.rs` `LaneRanking::consolidate` keys hits
  by `OccurrenceId` and keeps the lane's best score.
- `crates/retrieval/src/fusion/rrf.rs` `fuse` reads one entry per occurrence
  per lane into a union map slot indexed by lane, so a lane can contribute at
  most one term.
- `crates/retrieval/tests/fusion.rs` doubles and shuffles every lane's hits and
  compares the fused ranking bit for bit.

## Failure scenario

A lexical probe set matching one occurrence three times would give it three
terms and rank it above a single stronger match.

## Timing windows and dependencies

None. Fusion is a pure function over values.

## What a test must construct

- Random lane sets under a fixed seed, each hit duplicated, lanes shuffled.
- The per-probe negative control: `duplicates * term` differs from `term`.

## Investigation log

### Q: Does the negative control detect a per-probe vote?

- Sources examined: the property test's control arithmetic.
- Findings: whenever a duplicate exists and the lane weight is positive, the
  per-probe sum differs bitwise from the single term.
- Missing evidence: none.
- Conclusion: resolved with answer - yes.

# fusion-raw-scores-retained

## Discovery trigger

RP2.7 requires raw lane scores retained beside the fused score and never
compared across lanes.

## Evidence trail

- `FusedEntry::lane` returns the `LaneContribution` copied from the lane
  entry; `RawScore` is a lane-tagged enum with no cross-variant ordering.

## Failure scenario

Losing raw scores would make a fused position unexplainable and would tempt a
consumer to compare a lexical rank with a dense similarity.

## Timing windows and dependencies

None. Fusion is a pure function over values.

## What a test must construct

- Lanes with distinctive raw values checked after fusion and after a filter.

## Investigation log

### Q: None.

- Sources examined: none needed.
- Findings: none.
- Missing evidence: none.
- Conclusion: resolved with answer - no open question.

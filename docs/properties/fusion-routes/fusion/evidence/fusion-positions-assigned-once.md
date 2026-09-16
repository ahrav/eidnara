# fusion-positions-assigned-once

## Discovery trigger

RP2.7 KTD2 fixes probe consolidation by best lane rank then stable ID with
one re-rank, and fixes that revalidation filters without recomputing
positions.

## Evidence trail

- `LaneRanking::consolidate` assigns `1..=n` after sorting.
- `fuse` assigns fused positions `1..=n` after sorting and `Fused::filter`
  uses `retain`, which never touches a survivor.

## Failure scenario

A filter that renumbered survivors would make the selection digest depend on
which entries eligibility removed.

## Timing windows and dependencies

None. Fusion is a pure function over values.

## What a test must construct

- Lane sets with ties; an exact set; a filtered fused result.

## Investigation log

### Q: Should fused positions be renumbered after a filter?

- Sources examined: RP2.7 "Fusion runs once; final canonical revalidation
  filters the fused set and does not recompute scores or positions".
- Findings: renumbering is recomputation.
- Missing evidence: none.
- Conclusion: resolved with answer - positions keep gaps.

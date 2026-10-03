# dense-scan-bounds-end-with-no-candidate

## Discovery trigger

RP2.6 resources and #610 AC5: enforce scan work, batch rows and bytes, and
heap entries and bytes before work or allocation; exhaustion is a typed
non-result.

## Evidence trail

- `oracle::walk` refuses `HeapOverBound` when the preallocated slots exceed
  the heap bound and `BatchOverBound` for a pool past the kernel batch, before
  any page is read.
- `Block::flush` checks batch bytes before a selected candidate is pushed
  (`oracle.rs:652`); `Progress::hold` checks heap bytes, net of the entry a
  full set displaces, before an eligible row moves in (`oracle.rs:876`).
- `select_candidates` clears the pool for every incomplete reason except a
  coverage shortfall.

## Failure scenario

A batch or set that grows past its bound exhausts memory; a pool cut at a
bound and labeled complete omits neighbors.

## Timing windows and dependencies

Cancellation after a page is judged.

## What a test must construct

- Each bound at the fixture's need and one unit below it.

## Investigation log

### Q: Is a coverage shortfall a non-result?

- Sources examined: the f32 oracle's coverage contract.
- Findings: the pool is complete over the rows that carry codes; a shortfall
  means some live rows carry none yet, which the route reports as incomplete.
- Missing evidence: owner confirmation for the compressed lane.
- Conclusion: kept with the pool and marked incomplete; needs Q6 owner
  confirmation.

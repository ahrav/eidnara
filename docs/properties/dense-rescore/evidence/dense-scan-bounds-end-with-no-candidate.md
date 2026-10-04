# dense-scan-bounds-end-with-no-candidate

## Discovery trigger

RP2.6 resources and #610 AC5: enforce scan work, batch rows and bytes, and
heap entries and bytes before work or allocation; exhaustion is a typed
non-result.

## Evidence trail

- `check_request` refuses `HeapOverBound` when the preallocated slots exceed
  the heap bound and `BatchOverBound` for a pool past the kernel batch, before
  any page is read.
- `Progress::read_candidate` checks batch bytes from the row's strings before
  the batch candidate is allocated, and ends the batch at a row that does not
  fit; `Progress::hold` checks heap bytes before an eligible row moves in.
  Rows enter best first, so no entry is displaced.
- `Stored::batch_slots` (`crates/retrieval/src/dense/oracle.rs:647`) caps the
  candidate and score slots a batch reserves at `batch_bytes /
  SELECTED_ROW_BYTES`, so the reservation fits inside the bound before any
  row of the batch is read.
- `selected_bytes` counts each judged row's candidate, its score, and its
  strings.
- `select_candidates` clears the pool for every incomplete reason except a
  coverage shortfall.

## Failure scenario

A batch or set that grows past its bound exhausts memory; a pool cut at a
bound and labeled complete omits neighbors.

## Timing windows and dependencies

Cancellation after a page is visited.

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

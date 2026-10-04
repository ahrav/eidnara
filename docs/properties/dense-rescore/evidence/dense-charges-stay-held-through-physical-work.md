# dense-charges-stay-held-through-physical-work

## Discovery trigger

RP2.6 resources and #620 AC4: owned permits, pins, and scratch stay charged
until blocking work exits and is joined.

## Evidence trail

- The route clones `DenseVectors`, which holds the view's `Arc`, before
  `run_unit` for an `Embedded::Vector` request (`query_route.rs:1977`), and
  moves it into the unit's closure; an undeclared or unavailable embedding
  takes no clone.
- `rank_compressed` holds its `Scratch` and `RowBuffers` reservations for the
  whole call; the view's tables and pins live while any `Arc` does. `Scratch`
  is sized by `scan_scratch_bytes` (`crates/daemon/src/vector_reader.rs:303`):
  per layer one decoded block, the query encoded under the layer's scales, and
  the layer's code window, which is every byte the scan retains per layer.
- `SharedBudget::bridge` awaits the unit after cancellation instead of
  dropping it.

## Failure scenario

A request answered at cancellation while its worker still reads lets the next
request reserve the same bytes.

## Timing windows and dependencies

Between the client's cancellation and the worker's return.

## What a test must construct

- A real host, a held observer on the ranking thread, a client cancellation,
  and the error frame's publication time.

## Investigation log

### Q: Is the error published before the work returns?

- Sources examined: the host's publish hook in the test.
- Findings: no frame is published while the work is held; the frame follows
  the release.
- Missing evidence: none.
- Conclusion: resolved with answer - no.

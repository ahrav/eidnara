# dense-charges-stay-held-through-physical-work

## Discovery trigger

RP2.6 resources and #620 AC4: owned permits, pins, and scratch stay charged
until blocking work exits and is joined.

## Evidence trail

- The route clones `DenseVectors`, which holds the view's `Arc`, before
  `run_unit` for an `Embedded::Vector` request (`query_route.rs:1997`), and
  moves it into the unit's closure; an undeclared or unavailable embedding
  takes no clone.
- `rank_compressed` holds its `Scratch` and `RowBuffers` reservations for the
  whole call; the view's tables, mapped codes, and pins live while any `Arc`
  does. `Scratch` is sized by `scan_scratch_bytes`
  (`crates/daemon/src/vector_reader.rs:219`) as the scan's own payload
  buffers: per layer one decoded block and the query encoded under the
  layer's scales. Every scan over a view reads the code mappings its layers
  hold (`crates/daemon/src/vector_mapped.rs`), so acquisition charges the
  members' code files once as `MappedCodes` (`vector_reader.rs:422`) and the
  view keeps that reservation until its last layer's mapping is dropped;
  scans in flight together share it, as
  `scans_in_flight_together_charge_the_view_codes_once` shows. A production
  resident limit must admit every member's whole code file for a view to
  acquire. A code file truncated while a scan holds its mapping, or a device
  error behind a mapped page, raises `SIGBUS` and ends the host process,
  under the cooperative-file threat model the reader states. The
  per-layer descriptor vectors `rank_compressed` builds
  (`vector_reader.rs:819-833`) and the scan's control structures are bounded
  by `max_layers` and `max_entries` and stay outside the charge.
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

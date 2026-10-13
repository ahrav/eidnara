# dense-producer-limits-are-checked-at-installation

## Discovery trigger

RP2.6 and #620 AC3: every approved limit has an enforcement point; invalid
capacity configuration is refused.

## Evidence trail

- `CompressedLimits::capacity` derives the pool through
  `CandidateCapacity::new` and checks it and the scan page against the
  kernel's batch.
- `HandlerCore::set_dense_vectors` refuses without dense limits, refuses a
  route `unit_norm_tolerance` that differs from the view's layout, and runs
  the capacity check; `set_query_route_limits` repeats the pair check against
  installed vectors; `CompressedProducer::rank` runs the capacity check again
  per request.
- Byte, row, layer, pinned-byte, and read bounds are enforced in
  `select_candidates` and `rank_compressed`.

## Failure scenario

An unapproved pool size reaches a request and allocates or judges past the
kernel's batch.

## Timing windows and dependencies

None.

## What a test must construct

- Malformed alpha, an oversized pool, oversized pages, and missing dense
  limits.

## Investigation log

### Q: Are the production values approved?

- Sources examined: #825 D23 and D27.
- Findings: `k` 64 and a pool of 256 are named there; RP2.9 approval is Q6.
- Missing evidence: owner approval.
- Conclusion: unresolved, needs Q6 owner approval.

### Q: Does installation check the entry bound?

- Sources examined: `check_pair` (`crates/daemon/src/query_route.rs:758`),
  `CompressedLimits::capacity` (`query_route.rs:618`), `rank_compressed`
  (`crates/daemon/src/vector_reader.rs:808`, `:810`, `:836`), and
  `crates/daemon/tests/query_route_compressed.rs`
  `an_entry_bound_below_the_view_degrades_the_lane_before_a_scan`.
- Findings: installation checks `k`, the alpha policy, `candidates`, and
  `scan_page_rows`. `max_entries` reaches the resolver inside the request's
  scan, after `Scratch` and `RowBuffers` are reserved, so a composition whose
  entry bound is below its rows installs and every request degrades as
  `layers`.
- Missing evidence: an owner decision on whether installation refuses an
  entry bound below the view's rows.
- Conclusion: needs human input.

# route-bounds-are-enforced-before-protected-work

## Discovery trigger

The RP2.7 specification lists query bytes, probes, lane candidates, fused
union, canonical validation batch, materialization rows and bytes, and
serialized output as bounds that must be enforced before the protected work,
each with a saturation witness.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u2-weighted-rrf` at
`6dea07f455d536eec50556116c882c8dc6a00d98`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/query_route.rs`: `QueryRouteLimits` names every bound
  and `validate` refuses a `validation_batch` or `lexical_accepted` over the
  kernel's candidate maximum at installation; `handle_retrieval_query` checks
  `query_bytes` before the budget is derived; `execute` passes `query_bytes`
  to `classify`, refuses more `id:` selectors than `probes` before the first
  page, passes `probes` and `query_bytes` to `analyze_segments`,
  `probes`/`lexical_scan_rows`/`lexical_accepted`/`validation_batch` to
  `retrieve`, `exact_page_rows` and `exact_pages` to the exact loop,
  `fused_union` to `fuse`, judges revalidation in `validation_batch` slices,
  and applies `result_rows`/`response_bytes` in the materialization loop,
  which measures each entry with `measure_json` before pushing it.
- `crates/daemon/tests/query_route.rs`
  `each_bound_saturates_before_its_protected_work`.

## Failure scenario

A phase reads or serializes past the approved envelope under one budget, or
a bound is checked only after its work has run.

## Timing windows and dependencies

None.

## What a test must construct

- A projection whose matching rows exceed the shrunken bound.
- One limit shrunk at a time; the `before_phase` hook to assert the phase not
  reached.

## Investigation log

### Q: What is the typed outcome of a fused union over its bound?

- Sources examined: `retrieval::fusion::fuse` returns `UnionExceeded`; the
  RP2.7 terminal set has no union-specific member.
- Findings: the union bound is reached before materialization, so no payload
  is read; the closed terminal set leaves `lane_unavailable` as the lane-set
  failure.
- Missing evidence: none.
- Conclusion: resolved with answer - `lane_unavailable`, recorded as a Q5
  mapping in the catalog scope.

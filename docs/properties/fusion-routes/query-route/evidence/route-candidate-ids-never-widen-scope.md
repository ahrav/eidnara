# route-candidate-ids-never-widen-scope

## Discovery trigger

The RP2.7 specification requires that no user-supplied identifier expands
the bound project authorization and that revalidation under the bound scope
is the only path to a returned payload.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u2-weighted-rrf` at
`6dea07f455d536eec50556116c882c8dc6a00d98`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/query_route.rs`: `QueryRequest` is
  `deny_unknown_fields` and carries no identifiers; `execute` takes the
  `ProjectScope` from `RouteScope` and passes it as one `Authority` to
  `admit_exact`, to `lexical::admit`, and to the revalidation's
  `judge_tracked`.
- `crates/daemon/src/query_route.rs` `admit_exact` judges the exact lane's
  rows before `LaneRanking::consolidate`, so a row outside the scope never
  receives a lane position and never enters the union `fuse` bounds.
- `crates/daemon/tests/query_route.rs`
  `revalidation_excludes_retired_and_foreign_occurrences` foreign-scope
  assertion and `exact_lane_positions_and_bounds_count_only_eligible_rows`;
  `crates/daemon/tests/query_route_handler.rs` unknown-field assertion.

## Failure scenario

A caller names an occurrence from another project and receives its position
or payload, or reads from a gap in the returned positions how many rows of the
named object exist outside its scope.

## Timing windows and dependencies

None.

## What a test must construct

- A projection populated from one project; a foreign `ProjectScope`.
- A request body carrying an `occurrence_ids` field.
- Two named objects of which one is retired after the projection snapshot, so
  the exact lane reads rows the kernel refuses.

## Investigation log

### Q: Why does the foreign-scope answer say `fused` rather than a terminal?

- Sources examined: `execute`; the lanes' completion under a scope that owns
  nothing.
- Findings: both lanes ran and completed; the kernel excluded every entry.
  An empty fused answer is the truthful outcome and reveals nothing about the
  excluded rows.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Why are the exact lane's rows judged before ranking?

- Sources examined: `exact::page`, which filters on family, namespace, and key
  range and carries no scope; `LaneRanking::consolidate` and `fuse`, which
  number every hit they receive; `Fused::filter`, which keeps positions;
  `lexical::admit`, which ranks only admitted hits.
- Findings: with unjudged rows in the ranking, a survivor's position counted
  rows the caller may not see, so a position gap disclosed how many hidden,
  foreign, or retired rows a named object has, and those rows consumed
  `exact_pages` and `fused_union`. Judging before ranking removes both effects
  and makes the exact lane's positions mean the same as the lexical lane's.
- Missing evidence: none.
- Conclusion: resolved with answer.

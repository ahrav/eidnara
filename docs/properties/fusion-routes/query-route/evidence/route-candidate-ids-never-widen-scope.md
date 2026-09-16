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
  `ProjectScope` from `RouteScope` and passes it to `retrieve` through
  `Authority` and to `judge_occurrences_within_budget`.
- `crates/daemon/tests/query_route.rs`
  `revalidation_excludes_retired_and_foreign_occurrences` foreign-scope
  assertion; `crates/daemon/tests/query_route_handler.rs` unknown-field
  assertion.

## Failure scenario

A caller names an occurrence from another project and receives its position
or payload.

## Timing windows and dependencies

None.

## What a test must construct

- A projection populated from one project; a foreign `ProjectScope`.
- A request body carrying an `occurrence_ids` field.

## Investigation log

### Q: Why does the foreign-scope answer say `fused` rather than a terminal?

- Sources examined: `execute`; the lanes' completion under a scope that owns
  nothing.
- Findings: both lanes ran and completed; the kernel excluded every entry.
  An empty fused answer is the truthful outcome and reveals nothing about the
  excluded rows.
- Missing evidence: none.
- Conclusion: resolved with answer.

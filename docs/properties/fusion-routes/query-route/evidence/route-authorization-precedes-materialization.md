# route-authorization-precedes-materialization

## Discovery trigger

The RP2.7 specification requires authorization before candidate
materialization and denial of a wrong project before any text load; the U3b
ticket adds the harness comparison (parent Q3).

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u2-weighted-rrf` at
`6dea07f455d536eec50556116c882c8dc6a00d98`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/kernel_routes/mod.rs` `kernel_request` runs
  `kernel_route_scope` (session binding, `project_root`,
  `ProjectBinding::accepts`) before `parse_request_body`.
- `crates/daemon/src/query_route.rs` `handle_retrieval_query` calls
  `kernel_request::<QueryRequest>`, then `harness_for_route`, then compares
  the optional `harness` claim, then reads the limit set; the budget, the
  lifecycle pin, and every projection read come after.
- `crates/daemon/src/lib.rs` `harness_for_route` reads the bound
  `SessionBinding::harness`.
- `crates/daemon/tests/query_route_handler.rs`
  `scope_harness_and_disable_are_decided_before_any_candidate_read`.

## Failure scenario

A request with a foreign `project_root` or a borrowed harness name reads
another project's rows or inherits another harness's capabilities.

## Timing windows and dependencies

None; every check reads bound route state.

## What a test must construct

- A bound route on a `KernelDaemon`; a sibling project root; an unknown
  session id; a mismatched, a matching, and an absent harness claim.
- Limits installed, so the harness comparison is the refusing stage.

## Investigation log

### Q: Should the harness claim be required?

- Sources examined: `RouteIdentity` in `crates/host-runtime/src/handler.rs`,
  which calls its fields unverified claims; the `kernel.*` routes, which take
  no harness field.
- Findings: the binding already fixes the harness; a request field can only
  agree or disagree with it.
- Missing evidence: none.
- Conclusion: resolved with answer - the binding is authoritative, a
  disagreeing claim is refused, an absent claim is accepted.

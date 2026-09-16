# route-rollback-disables-without-mutating-canonical-truth

## Discovery trigger

The RP2.7 specification fixes the default rollback as disabling the route
while retaining canonical state; no rollback mutates canonical truth to match
a projection.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u2-weighted-rrf` at
`6dea07f455d536eec50556116c882c8dc6a00d98`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/query_route.rs` `set_query_route_limits(None)` clears
  the limit set; `handle_retrieval_query` answers `disabled` right after the
  binding resolves and before the budget, the pin, or any projection or
  canonical read. `kernel_route_scope` acquires the store handle first, so a
  request during the store's `Starting` or unavailable phase receives that
  kernel `state` answer instead of `disabled`.
- `crates/daemon/src/lib.rs` `HandlerCore::query_route` starts as `None`.
- `crates/daemon/tests/query_route_handler.rs` disable and re-enable
  assertions in both tests.

## Failure scenario

A rollback that edits canonical rows to match a projection cannot be undone.

## Timing windows and dependencies

The daemon's store-starting window: the binding answers `state` before the
limit set is consulted. The tests run against a ready daemon.

## What a test must construct

- A converged family so the enabled answer is `fused`.
- Disable, then re-enable, with no data change between.

## Investigation log

### Q: Is a disabled route distinguishable from a missing one?

- Sources examined: the dispatch arm in `crates/daemon/src/lib.rs`; the
  transport's unknown-method error.
- Findings: the arm always exists; the `disabled` terminal tells a harness the
  route is known but switched off.
- Missing evidence: none.
- Conclusion: resolved with answer.

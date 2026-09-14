# request-conversion-completion-ownership

## Discovery trigger

Cancellation, route close, and shutdown cannot return a block or its charge before the barrier-held copy/decode physically completes. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/host-runtime/src/handler.rs:606`
- PR #518 `c70367615f7e391d7786395b58fb71b4a43bc47e`

Witness status: not yet - the request-scoped copy/conversion seam over the owned lease is #548's implementation; the join owner from PR #518 exists in `crates/host-runtime/src/handler.rs` (`RequestCtx::run_blocking`).

## Failure scenario

An early return would reuse a block a worker is still copying.

## Timing windows and dependencies

Cancel, route close, and shutdown during copy.

## What a test must construct

A barrier holding copy work while `Cancel` arrives.

Situation markers that must fire independently of the safety check:

- `host.cancel_during_barrier_held_copy`

Check semantics: `always` - a lease moved into blocking work returns only after that work joins; `Cancel` observed mid-copy leaves `outstanding_returns` unchanged until the join.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: not yet at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: #548.
- Conclusion: unresolved, needs the named handoff.

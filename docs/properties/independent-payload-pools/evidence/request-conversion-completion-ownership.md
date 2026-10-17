# request-conversion-completion-ownership

## Discovery trigger

Cancellation, route close, and shutdown cannot return a block or its charge before the barrier-held copy/decode physically completes. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/host-runtime/src/handler.rs:617`
- `crates/host-runtime/src/dispatch.rs:1047`

Witness status: partial - `crates/host-runtime/src/ring_transport.rs:4010` holds a real `into_private` copy on the blocking barrier while the request, route, and host ledgers close, and shows `outstanding_returns` and the ingress charge unchanged until the copy joins, then each returned once. `crates/host-runtime/tests/dispatch.rs:749` and `crates/host-runtime/tests/dispatch.rs:805` drive the production Cancel and route-close paths against handler blocking work (`blocking_hold`), which starts after `dispatch_request` has already completed the inbound copy; no test pauses the production copy itself under Cancel, route close, or shutdown.

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
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.

### Q: Is the handoff closed at the closing-evidence head?

- Sources examined: the tests named in `Exercised` and the code they drive.
- Findings: The test-support `inbound_copy_gate` holds the production copy in `dispatch_request` for bodies of one length. Under the hold, `Cancel`, a route close, and shutdown each wait, `host.status` reports one outstanding return, and release settles the request as `cancelled` with the block returned.
- Missing evidence: None for this record.
- Conclusion: resolved with answer - the record is exercised.

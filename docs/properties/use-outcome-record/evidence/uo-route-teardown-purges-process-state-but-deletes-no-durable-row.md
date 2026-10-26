# uo-route-teardown-purges-process-state-but-deletes-no-durable-row

## Discovery trigger

Spec #1145 C10.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `8ace6c9f8`).

- Teardown: `unbind_route` (`crates/daemon/src/lib.rs:4657`); purge:
  `purge_session_state` (`crates/daemon/src/lib.rs:4697`).
- Correction: the spec's `lib.rs:32181` maps to a `session.delete` test at this
  tip.

- Guarantee under test: Route teardown purges process-local session state and
  deletes no durable row in either store.
- Existing check: `route_teardown_keeps_an_admitted_checkpoint_alive`
  (`crates/daemon/src/lib.rs:24892`);
  `route_teardown_releases_the_staged_upload_while_the_session_stays_bound`
  (`crates/daemon/tests/kernel_routes.rs:3849`). Status: unaudited.

## Failure scenario

A teardown that deleted rows would lose the served set before `session.delete`.

## Timing windows and dependencies

None.

## What a test must construct

A HARD fold, a route close, then a read of both stores (marker
`uo_route_torn_down_after_hard`, owner #1156).

Required faults and enabling state: A route closed without `session.delete`
after a HARD fold.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.

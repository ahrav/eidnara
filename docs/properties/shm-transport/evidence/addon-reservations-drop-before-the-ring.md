# addon-reservations-drop-before-the-ring

## Discovery trigger

The native addon's `Channel` holds producer reservations and receive leases that borrow ring memory, and hands JavaScript `Uint8Array` views over that memory. If the ring were dropped before those borrowers, a finalizer would touch unmapped memory.

## Evidence trail

- `Channel` (`packages/shm-native/src/lib.rs:69-88`) declares `producers`, `active`, and `stranded` before `to_host` and `from_host`; the field comment states the order is load-bearing because Rust drops fields in declaration order.
- `close_channel` (`lib.rs:407-413`) detaches every producer, every active lease, and every stranded alias with `?`, so a detachment failure returns before the entry is removed.
- `close` (`lib.rs:1546-1561`) removes the registry entry only when `producers`, `active`, and `stranded` are all empty; otherwise the entry and its mapping stay registered and a later `close` retries.
- `channel_drops_borrowing_reservations_before_the_ring` (`lib.rs:1282`) builds a `Channel` holding a reservation that borrows `to_host` and drops it; the test passes only if the reservation is destroyed first.
- `packages/shm-native/tests/runtime.ts` asserts detachment under Bun and records `detachment_unavailable` under Node.
  At HEAD: `close_channel` delegates to `detach_all_aliases` (`:386-405`), which does not stop at the first failure: it detaches every producer, lease, and stranded alias and returns the first failure afterwards.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

A field reorder, or a new borrowing field declared after the rings, so a finalizer runs against unmapped memory. Externally, a `Uint8Array` that stays attached after a successful `close`.

## Timing windows and dependencies

None at close. The harm lands later, when the JavaScript finalizer runs.

## What a test must construct

- Present: the drop-order unit test and the Bun detachment cases.
- Missing: the external half under Node, which reports detachment unavailable and is therefore a capability refusal rather than a proof; and a test that a failed detachment keeps the entry registered.

## Investigation log

### Q: Does a detachment failure violate the guarantee?

- Sources examined: `close`, `close_channel`, the field comment on `Channel`.
- Findings: On a detachment error `close` returns the error and keeps the entry and mapping registered by design, so the view stays valid until a later `close` succeeds.
- Missing evidence: A test that drives the failure path.
- Conclusion: resolved with answer: the check is scoped to `close` returning `Ok`; the error path is the designed quarantine.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 10, `lib.rs:361-376` now `lib.rs:407-413`: At HEAD `close_channel` delegates to `detach_all_aliases` (`:386-405`), which does not stop at the first failure: it detaches every producer, lease, and stranded alias and returns the first failure afterwards.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.

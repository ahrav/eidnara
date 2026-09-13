# served-order-decision-visits-every-object

## Discovery trigger

Plan KTD1 rejects short-circuit aggregation. A local changed flag is not
enough: the copier later depends on every object's field order being ready.
The canonicalizer lane identifies this obligation; wildcard inspection adds
a production ordered-parent/disordered-child witness.

## Evidence trail

- [The real loop][encode] sorts every object before copying at HEAD.
- [Recording and copying][tables] keep objects in source-start order while
  each object's field vector can move. Recursion depends on that separation.
- [WireBlockData][block] emits `kind`, then optional `provider_extras`, which
  is ordered. [BlockKind][kind] emits the `type` tag before fields such as
  `text`, so a child can be disordered while its parent is ordered.
- [Typed WireMessage][message] emits `role` before `content`. A retained
  original replays Value instead. Under sorted Value iteration, the exact
  ordered-message-root/disordered-child fixture needs private generic input.
- [Nested literal tests][tests] are unaudited source evidence, not executions.

All numbered anchors were verified with `git show HEAD:<path>` at
`2e4433e6b511ae74944df8a9669c428e73915d29`. Plan baseline `4980f8af` is not
this source revision. No candidate or mutation ran.

## Failure scenario

`changed || sort_fields(...)` can stop evaluating after an early inversion.
Later objects then retain source field order and B copies incorrect bytes.
A root-only decision also misses child disorder even when no parent key moves.

## Timing windows and dependencies

The dependency is synchronous finalization order, not a thread interleaving.
Every recorded object needs exactly one sort invocation; object ranges and
object-table order must remain unchanged. The aggregate equals the OR of
independently expected local permutation changes only after the full loop.

## What a test must construct

Use the existing module's generic input support and real production shells.
Include an early disordered object and at least two later disordered siblings
or descendants. Interleave ordered, empty, and singleton objects so merely
counting changed objects cannot stand in for visiting all objects.

Add a typed block whose parent order is canonical and whose tagged child has
an inversion. Separately construct the exact ordered-root/child-only case
privately; label that witness test-only under the pinned feature assumptions.
The production block case requires child traversal, but the typed message
root already has an inversion. It cannot isolate a false-negative root-only
aggregate decision; only the private ordered-root/child-only case does that.
Use literal expected bytes or decoded emitted-key ordering, not a Value
round trip that silently sorts away the input situation.

Verify the actual loop is non-short-circuiting and use late-disorder byte
witnesses to reject a skipped-sort mutation. These observations are enough
without per-object production counters. If test-only visits are recorded,
compare per-object identities, not only a total that can hide duplicate work.
Never build a second test loop and mistake it for the production loop.

## Investigation log

### Q: Must exact visitation require new per-object instrumentation?

- Sources examined: the real finalization loop and copier.
- Findings: source verification plus discriminating late-disorder literals
  can expose the targeted short-circuit failure with less apparatus.
- Missing evidence: candidate loop review and executed negative control.
- Conclusion: resolved with answer. Preserve the invariant; do not mandate
  counters or a production API to observe it.

### Q: Is every ordered-parent example test-only?

- Sources examined: WireMessageData, WireBlockData, and BlockKind.
- Findings: the block-parent witness is production-reachable. The exact
  WireMessage-root witness has the separate private-test limitation above and
  alone isolates the false-negative root-only aggregate.
- Missing evidence: constructed and executed witnesses at this revision.
- Conclusion: resolved with answer. Route this record to `/testing:test-strategy`
  and existing tests to `/testing:invariant-test-review`.

[encode]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L121-L142
[tables]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L42-L109
[block]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L243-L278
[kind]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L326-L339
[message]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L114-L162
[tests]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L170-L252

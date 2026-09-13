# referenceable-json-rejects-hooks-before-reading

## Discovery trigger

The parent specification's TE19 requires source validation without invoking
accessors, proxies or serialization hooks. Recursive field snapshots must not
invoke those hooks while attempting to detect mutation.

This evidence covers the client source guard. The parent companion catalog is
unavailable in this checkout. No claim about its full acceptance status is
inferred from this supplement.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner`, parent `b0023512`. Line
numbers below are from that tree.

- [transform-capture.ts](../../../../../packages/opencode-plugin/src/hooks/context/transform-capture.ts):
  `ReferenceableWalk.entries` rejects proxies with `util.types.isProxy` before
  any prototype or descriptor read (`:207`), checks the prototype, depth, and
  cycles (`:208-218`), walks the prototype chain for an own or inherited
  `toJSON` (`:220-236`), and charges the declared array length before element
  reads (`:240-241`). `data` spends one slot per descriptor read and rejects
  sparse slots and accessors (`:192-198`). `walk` rejects functions, symbols,
  bigints, and non-finite numbers (`:137-147`). `readOwnDataProperty` (`:70`)
  returns `undefined` for accessors, proxies, and inherited properties.
- [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts):
  `execute` reads `output.messages` with `readOwnDataProperty` (`:844`),
  checks the container (`:845`), inspects the source before any message read
  (`:967`), and captures it (`:977`). The candidate returned by the daemon,
  including host-owned kept prefix entries, is inspected before
  `assertNativeBoundary` and any plain read (`:1436-1445`).
- [module-wire.ts](../../../../../packages/opencode-plugin/src/hooks/context/module-wire.ts):
  `primeOrdinalMemo` (`:473`) scans asynchronously; the caller rechecks the
  capture (`rust-mode-transform.ts:1167`) before `annotateOrdinals` reads
  messages synchronously.
- [hook.ts](../../../../../packages/opencode-plugin/src/hooks/context/hook.ts):
  the session ID is read through nested `readOwnDataProperty` calls
  (`:96-97`) and the transform entry reads `output.messages` the same way
  (`:339`).
- [messages-transform.ts](../../../../../packages/opencode-plugin/src/plugin/messages-transform.ts):
  `returnableMessageArray` (`:13-22`) checks proxy, array, prototype chain,
  and `then` before the hook and again before returning (`:59-63`, `:76-82`).
  The wrapper never assigns `output.messages`.

## Failure scenario

A getter installed on `parts` during an ordinal scan can execute before a
post-resolver guard runs. The resolver therefore rechecks before its
synchronous source-reading half. A proxy root can trap even during length or
membership inspection; the non-trapping runtime proxy predicate runs first.

## Timing windows and dependencies

The direct tests distinguish initial rejection from installation after an
earlier valid dispatch. They pause directory and transport promises and
schedule installation from the ordinal provider or a post-response microtask.
Each pause window has an unmutated control that publishes. Pure helpers also
test content edits, membership replacement, removal, metadata renames and
descriptor changes.

## What a test must construct

- Construct an unsupported root, member or nested value and count every getter
  and proxy trap independently of dispatch counts.
- Demonstrate a successful readonly-input transform with exact approved output
  and preserved destination and payload identity.
- Capture supported input, reach the named await, then install the hook.
- Preserve optional undefined object fields and harmless hidden data as positive
  controls; change hidden data and require the recheck to fail.
- Exercise cumulative walk exhaustion and acyclic reference amplification.

## Investigation log

### Q: What has executable evidence?

- Sources examined: the source paths above and the witnesses below.
- Findings: Every guard entry has a trap-counter witness. The non-trapping
  property of `util.types.isProxy` is a Node runtime behavior; these tests
  establish it on Node 24.18.0 under Bun 1.3.14 only.
- Missing evidence: Native-addon-enabled CI coverage and U5 measurements are
  outside this run. Earlier review findings and repairs are recorded in
  [the dispositions](../review-dispositions.md).
- Conclusion (2026-09-13, revision-bound run, 1098 pass, 0 fail): resolved
  as exercised. Witnesses, all with marker `expect(counter.count).toBe(0)`,
  `expect(trap).not.toHaveBeenCalled()`, or a zero `getterCalls` or
  `trapCalls` count:
  - `transform-capture.test.ts:75`, `:107`, `:212`, `:282`, `:291`, `:305`,
    `:338`, `:364`, `:549`, `:891`, and the `it.each` families at `:132`
    (own array `toJSON`), `:154` (hidden array operation overrides), `:168`
    (membership accessors), `:185` (inherited `toJSON`), `:249` (hidden
    accessors on production-read fields).
  - `rust-mode-transform.test.ts:2581` "declines an unsupported source before
    any dispatch and leaves the host array intact" (`calls` 0); `:2387`
    "rejects nested <accessor|toJSON|proxy> installed at <source-await|
    pre-apply> without invoking it" (6 cases); `:1980` "refuses a kept
    previous-output entry that gained an accessor and invokes no hook".
  - `hook.test.ts:210` "rejects <unsupported> at the actual <hook|wrapper>
    entry without triggering reads" (12 cases; also `client.session.get` and
    `fake.calls` untouched); `hook.test.ts:174` `it.each` accepts hidden own
    data through both entries as a positive control.
  - `messages-transform.test.ts:64` and `:114` `it.each` families refuse
    `then`, root proxies, and prototype proxies at entry and after the await;
    `:196` leaves array-slot inspection to the inner owner.

Focused command, run from `packages/opencode-plugin` with Node 24.18.0 first on
PATH:

```sh
bun test src/hooks/context/ src/plugin/messages-transform.test.ts \
  src/shared/host-client/client.test.ts
```

Result: 1098 pass, 0 fail, 31,141 `expect()` calls, 34 files. `tsc --noEmit`
over `src/` passes in the same tree; the package `typecheck` script then fails
in `tsconfig.scripts.json` at `scripts/bench-transform-client.ts:206`
(`Expected 2 arguments, but got 3`), which is outside these records. Earlier
whole-repository gate claims from a prior revision are not repeated here.

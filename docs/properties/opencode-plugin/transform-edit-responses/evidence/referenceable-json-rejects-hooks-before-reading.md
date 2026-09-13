# referenceable-json-rejects-hooks-before-reading

## Discovery trigger

The parent specification's TE19 requires source validation without invoking
accessors, proxies or serialization hooks. Recursive field snapshots must not
invoke those hooks while attempting to detect mutation.

This evidence covers the client source guard. The parent companion catalog is
unavailable in this checkout. No claim about its full acceptance status is
inferred from this supplement.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner` after merging
`origin/main` at `5def3c71`. Line numbers below are from that tree.

- [transform-capture.ts](../../../../../packages/opencode-plugin/src/hooks/context/transform-capture.ts):
  `rootArrayRejection` (`:94-117`) returns a `ReferenceableRejection`
  (`{reason, path}`) for a proxy root (`:95`), a non-array root (`:96`), an
  altered prototype chain (`:97-102`), an own or inherited `then` (`:103`),
  or an accessor on `Array`, `Object`, `String`, `Number`, or
  `Boolean.prototype` (`:105-115`, reason `prototype_accessor`, path such as
  `Object.prototype/agent`). `ReferenceableWalk.members` (`:187-195`) calls it
  first, so `inspectReferenceableMessages` (`:345-363`), `captureMessages`
  (`:383-400`), `capturedMessagesUnchanged` (`:403-430`), and
  `hostArrayReplacementRejection` (`:439-455`) all refuse a polluted
  prototype.
- `entries` (`:237-334`) rejects proxies with `util.types.isProxy` before any
  prototype or descriptor read (`:242`), rejects boxed primitives with
  `util.types.isBoxedPrimitive` (`:244`), checks the prototype, depth, and
  cycles (`:245-255`), walks the prototype chain for an own or inherited
  `toJSON` using `Object.hasOwn(hook, "value")` (`:257-273`), charges the
  declared array length before element reads (`:277-278`), and records
  `prototype === null` on the tape for objects (`:285`). `data` spends one
  slot per descriptor read and rejects sparse slots and accessors with
  `Object.hasOwn(slot, "value")` (`:228-234`). `walk` rejects functions,
  symbols, bigints, and non-finite numbers (`:170-185`). `readOwnDataProperty`
  (`:72-76`) returns `undefined` for accessors, proxies, and inherited
  properties.
- [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts):
  `execute` reads `output.messages` with `readOwnDataProperty` (`:846`),
  checks the container (`:847-854`, logging `prototype_accessor` at warn),
  inspects the source before any message read (`:969-977`, `unsupported_source`
  at warn for `prototype_accessor`, debug otherwise), and captures it
  (`:980`). The daemon's candidate is not inspected before
  `assertNativeBoundary` (`:1433-1446`); kept-prefix validation is #538's
  TE21.
- [module-wire.ts](../../../../../packages/opencode-plugin/src/hooks/context/module-wire.ts):
  `primeOrdinalMemo` (`:473`) scans asynchronously; the caller rechecks the
  capture (`rust-mode-transform.ts:1172`) before `annotateOrdinals` reads
  messages synchronously.
- [hook.ts](../../../../../packages/opencode-plugin/src/hooks/context/hook.ts):
  the session ID is read through nested `readOwnDataProperty` calls
  (`:96-97`) and the transform entry reads `output.messages` the same way
  (`:339`).
- [messages-transform.ts](../../../../../packages/opencode-plugin/src/plugin/messages-transform.ts):
  the wrapper checks only the root array with `rootArrayRejection` at entry
  (`:59`) and return (`:77`) and logs `[eidnara] transform declined: <reason>
  at <path> (entry|return)` (`:17-21`), warn for `prototype_accessor` and
  debug otherwise. The wrapper never assigns `output.messages`.

## Failure scenario

A getter installed on `parts` during an ordinal scan can execute before a
post-resolver guard runs. The resolver therefore rechecks before its
synchronous source-reading half. A proxy root can trap even during length or
membership inspection; the non-trapping runtime proxy predicate runs first. An
accessor installed on `Object.prototype` runs on any read of an absent
optional field; the built-in prototype scan refuses the pass before such a
read.

## Timing windows and dependencies

The direct tests distinguish initial rejection from installation after an
earlier valid dispatch. They pause directory and transport promises and
schedule installation from the ordinal provider, a page callback, or a
post-response microtask. Each pause window has an unmutated control that
publishes. Pure helpers also test content edits, membership replacement,
removal, metadata renames, descriptor changes, and prototype pollution.

## What a test must construct

- Construct an unsupported root, member, nested value, or built-in prototype
  accessor and count every getter and proxy trap independently of dispatch
  counts.
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
- Conclusion: resolved as exercised; the witness list is under the next
  question.

### Q: Does the guard refuse built-in prototype pollution without reads?

- Sources examined: `rootArrayRejection` and `entries` at the lines above,
  the `prototype_accessor` log levels in `rust-mode-transform.ts:849` and
  `:975` and `messages-transform.ts:18`, and the witnesses below.
- Findings: Every guard entry has a trap-counter witness. The
  "built-in prototype scan" describe covers `Object.prototype`,
  `Array.prototype` (iterator symbol and `then`), and `String.prototype`
  accessors, boxed primitives, the null-prototype tape marker, and a 12,000
  message positive control. The daemon's candidate is not inspected before
  `assertNativeBoundary`; that path is TE21 in #538. The `Number.prototype`
  and `Boolean.prototype` entries of the scan have no dedicated witness.
- Missing evidence: A witness for an accessor on `Number.prototype` or
  `Boolean.prototype`; native-addon CI coverage as before.
- Conclusion (2026-09-13, revision-bound run after merging `origin/main` at
  `5def3c71`, 1107 pass, 0 fail): resolved as exercised. Witnesses, all with
  a zero trap, getter, or hook count:
  - `transform-capture.test.ts:76`, `:108`, `:213`, `:283`, `:292`, `:306`,
    `:339`, `:365`, `:760`, `:1094`, and the `it.each` families at `:133`
    (own array `toJSON`), `:147` (hidden array operation overrides), `:166`
    (membership accessors), `:183` (inherited `toJSON`), `:241` (hidden
    accessors on production-read fields).
  - "built-in prototype scan" (`:381`): `:384` "rejects an accessor inherited
    from Object.prototype without calling it" (`rejection` equals
    `{ reason: "prototype_accessor", path: "Object.prototype/agent" }`,
    `capturedMessagesUnchanged` false, then `undefined` after restore);
    `:409` "rejects an Array.prototype iterator accessor without calling it"
    (path `Array.prototype/Symbol(Symbol.iterator)`); `:428` "rejects an
    Object.prototype value accessor alongside a source accessor without
    calling either" (`readOwnDataProperty` returns `undefined`, both counters
    0); `:453` "rejects prototype-reset boxed primitives whose tapes cannot
    distinguish their values" (`boxed_primitive` at `/0/flag`); `:463`
    "rejects a String.prototype accessor without calling it"; `:481`
    "records whether a nested object has a null prototype"
    (`capturedMessagesUnchanged` flips with the prototype); `:491` "rejects
    an inherited then on the root array without calling it"
    (`extra_property` at `/then`, plus `proxy` and `not_array` roots); `:518`
    "accepts a metadata-heavy history of short messages within the walk
    budget" (12,000 messages, `estimatedBytes < 5 * jsonBytes`).
  - `:843` "rejects inherited membership and does not consult its getter"
    (getter on `Array.prototype["0"]`; `capturedMessagesUnchanged` false and
    `inspectReferenceableMessages` not ok); `:783` "refuses a numeric
    accessor on a built-in prototype and defines slots without invoking it"
    (2 cases; `inspectReferenceableMessages` reports `prototype_accessor` at
    `<Array|Object>.prototype/0`, `counter.count` 0).
  - `rust-mode-transform.test.ts:2575` "declines an unsupported source before
    any dispatch and leaves the host array intact" (`calls` 0); `:2375`
    "rejects nested <accessor|toJSON|proxy> installed at <source-await|
    pre-apply> without invoking it" (6 cases); `:2536` "declines before
    publication and NACKs known deliveries when the source changes between
    pages" (`hook` never called); `:2608` (accessor on `info` during the
    directory await, `getter` never called).
  - `hook.test.ts:210` "rejects <unsupported> at the actual <hook|wrapper>
    entry without triggering reads" (12 cases; also `client.session.get`,
    `client.app.agents`, and `fake.calls` untouched); `hook.test.ts:171`
    `it.each` accepts hidden own data through both entries as a positive
    control.
  - `messages-transform.test.ts:61` and `:110` `it.each` families refuse
    `then`, root proxies, and prototype proxies at entry and after the await
    and assert the debug log text; `:175` "logs a polluted built-in prototype
    at warn and skips the inner hook" (`warn` called with `transform
    declined: prototype_accessor at Object.prototype/agent (entry)`,
    `hookCalls` 0, `getterCalls` 0); `:232` "leaves array-slot inspection at
    <entry|await> to the inner owner" (2 cases).

Focused command, run from `packages/opencode-plugin` with Node 24.18.0 first on
PATH:

```sh
bun test src/hooks/context/ src/plugin/messages-transform.test.ts \
  src/shared/host-client/client.test.ts
```

Result: 1107 pass, 0 fail, 31,373 `expect()` calls, 34 files. `bun run
typecheck` (which includes `tsc --noEmit`, `tsconfig.scripts.json`, and
`tsconfig.tui.json`) exits 0 in the same tree. Earlier whole-repository
gate claims from a prior revision are not repeated here.

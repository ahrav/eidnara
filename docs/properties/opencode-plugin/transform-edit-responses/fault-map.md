# Fault map: client execution (U3)

Fault classes for the client supplement, with the executed witnesses that
construct each fault on the revision-bound run (the #533 change on
`fix/client-transform-owner` at `d5a525e8`, after merging `origin/main` at
`5def3c71`, 2026-09-13, 1122 pass, 0 fail). Test adequacy is not independently reviewed
here. This map supplements, and does not replace, the unavailable parent
companion's fault map. Test lines are in
`packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts`
unless another file is named.

| Fault class | Executed witness and enabling-state marker | Records |
| --- | --- | --- |
| Host mutates captured content during an await | `:2770`, `:2454` (8), `:2367` (8), `:2594` (4), `:2010` (mutation, accessor), `:803`, `hook.test.ts:360`; markers: `calls` or `bodies` length before the edit, `pageSizes`, page-zero body, `live[0]` replaced before the response | TE17, TE27, TE30 |
| Host mutates captured content between two transport pages | `:2660`; markers: `bodies.length > 1`, last body `transform_page_complete` true, `hook` never called, NACK list equals `delivered`. The series completes; publication refuses | TE17, TE18, TE20, TE22 |
| Host installs a hook after capture | `:2732`, `:2499` (6), `:2660`, `hook.test.ts:360`; markers: `started.promise` race, `directoryReached`, `transportProcessedBeforeInstall`; hook never called | TE17, TE19 |
| Unsupported source at entry | `:2699`, `hook.test.ts:210` (12), `transform-capture.test.ts:75-379`, `messages-transform.test.ts:61`, `:110`; marker: trap counters zero, `calls` 0 | TE19 |
| Accessor on a built-in prototype | `transform-capture.test.ts:384`, `:409`, `:428`, `:463`, `:491`, `:783`, `:843`, `messages-transform.test.ts:175`; markers: `rejection` equals `{ reason: "prototype_accessor", path }`, `counter.count` 0, `warn` log at entry | TE19 |
| Invalid container or over-budget candidate | `transform-capture.test.ts:779-886`, `rust-mode-transform.test.ts:2085` (2), `:2718`; marker: rejection reason named, host array identity kept, `native candidate array` decline log | TE20, TE23 |
| Session clear or wire invalidation mid-flight | `:2807`, `:2843`, `:2231` (clear, invalidate), `:2367` (clear, invalidation), `:2594` (clear, invalidation), `:2010` (invalidation), `:858`, `:980`; marker: `calls` 1 before the fault | TE18, TE20, TE22, TE26 |
| Newer same-session call with a settling owner | `:2992`, `:2231` (supersede), `:3170`, `:894`, `:941`, `hook.test.ts:396`; marker: `activePasses` 1 and held charge after the decline | TE23, TE24 |
| Count or byte pressure | `:2874`, `:2923`, `:1953`, `:2149`, `:2280`, `transform-capture.test.ts:889`, `hook.test.ts:301`; marker: exact `heldCharge`, `pass_count` log, `capture_bytes` warn log, blocker remainder | TE23, TE27 |
| ACK failure or pause | `:3025`, `:1290`, `:1344`; marker: `ackStarted.promise` race, `maxActiveCalls` 1 | TE22, TE23 |
| NACK pause with a following pass | `:3086`; marker: `nackStarted.promise` race, per-identity tuples | TE22, TE23 |
| Outcome-unknown send | `:3255`, `:2335`; marker: `calls` 1 and `failureCount` 1 after the throw | TE26, TE27 |
| Reconnect or attempt mismatch after a source or wire fault | `:2010` (6); positive controls `:594` (3), `:640`; marker: one page-zero body before the restart result and still one after it for every fault, `getterCalls` 0 | TE17, TE26 |
| Applied previous output mutates before advertisement or reuse | No witness. The candidate is not inspected before `assertNativeBoundary`. Kept-prefix validation is #538 | TE21 |
| Optional-output bytes fill before session count | No witness; `wireCaches` is count-bounded (64). TE25 is #538 | TE25 (outside this supplement) |
| Changed output followed by raw append | `:3025`, `:1344` dispatch a later delta; `:803` forces a full resend after a source-declined dispatch; no delta/full control | TE30 |
| Real transport abort through the lease signal | `:2231` uses an in-process client that rejects on abort; no daemon transport witness | TE24 |

## Enabling-state markers observed

These are assertions in the executed tests, not installed production markers.
Each fires on a correct implementation before the fault is introduced.

- A valid capture exists and a named await is paused: `started.promise`,
  `reachedPage1.promise`, `pageRead.promise`, `ackStarted.promise`,
  `nackStarted.promise` races against the pass; `expect(calls)` or
  `expect(bodies)` length before the fault; `expect(pageSizes).toEqual(
  [MODULE_ORDINAL_PAGE_SIZE])`; `transportProcessedBeforeInstall`.
- A paged series is in flight with page zero accepted: page zero body count
  1 and `transform_page_index` 1 pending, or the accessor installed when the
  fake receives `transform_page_index` 1 and the final body carrying
  `transform_page_complete` true.
- A dispatched delta exists whose daemon-side snapshot is committed: the fake
  replaces `live[0]` while producing its response, and `bodies[2].tail_delta`
  is defined.
- A count slot or byte budget is occupied: `activePasses` and exact
  `chargedBytes` while the first pass is paused; a blocker lease reserving
  `remainingBytes - <needed> + offset`.
- Cancellation requested while protected state is live: `signal.aborted` true
  with `activePasses` 1 and `chargedBytes > 0` before settlement.
- Publication completed while delivery is paused: `output.messages[0]` is the
  applied entry and counters are 0 at `ackStarted`.
- Settlement before recovery: `activePasses` 0 and `chargedBytes` 0, or the
  blocker's `release()`, before the single recovery `run`.

## Remaining work, by cheapest useful oracle

1. Independent adequacy review of the witnesses above
   (`/testing:invariant-test-review`), in particular the pre-apply microtask
   timing at `:2499`, the between-pages refusal at `:2660`, and the
   model-based test at `transform-capture.test.ts:889`.
2. A clear, invalidation, or supersession witness landed during the directory
   or permission await, which are `assertCurrentPass()` fences only.
3. TE21 and TE30 integration evidence with #538; TE25's separate byte budget.
4. Real transport abort and response-loss evidence where in-process fakes
   cannot answer the property.

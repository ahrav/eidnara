# Fault map: client execution (U3)

Fault classes for the client supplement, with the executed witnesses that
construct each fault on the revision-bound run (the #533 change on
`fix/client-transform-owner`, parent `b0023512`, 2026-09-13, 1098 pass, 0 fail). Test adequacy is
not independently reviewed here. This map supplements, and does not replace,
the unavailable parent companion's fault map. Test lines are in
`packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts`
unless another file is named.

| Fault class | Executed witness and enabling-state marker | Records |
| --- | --- | --- |
| Host mutates captured content during an await | `:2652`, `:2342` (8), `:2255` (8), `:2482` (4), `:2548`, `:1870` (2), `hook.test.ts:301`; markers: `calls` or `bodies` length before the edit, `pageSizes`, page-zero body | TE17 |
| Host installs a hook after capture | `:2614`, `:2387` (6), `hook.test.ts:301`; markers: `started.promise` race, `directoryReached`, `transportProcessedBeforeInstall`; hook never called | TE17, TE19 |
| Unsupported source at entry | `:2581`, `hook.test.ts:210` (12), `transform-capture.test.ts:74-378`, `messages-transform.test.ts:64`, `:114`; marker: trap counters zero, `calls` 0 | TE19 |
| Invalid container or candidate | `transform-capture.test.ts:568-675`, `:1916` (2), `:2600`, `:1980`; marker: rejection reason named, host array identity kept | TE20 |
| Session clear or wire invalidation mid-flight | `:2689`, `:2725`, `:2119` (clear, invalidate), `:2255` (clear, invalidation), `:2482` (clear, invalidation), `:826`, `:948`; marker: `calls` 1 before the fault | TE18, TE20, TE22 |
| Newer same-session call with a settling owner | `:2874`, `:2119` (supersede), `:3052`, `:862`, `:909`, `hook.test.ts:337`; marker: `activePasses` 1 and held charge after the decline | TE23, TE24 |
| Count or byte pressure | `:2756`, `:2805`, `:1813`, `:2037`, `:2168`, `transform-capture.test.ts:678`; marker: exact `heldCharge`, `pass_count` log, blocker remainder | TE23, TE27 |
| ACK failure or pause | `:2907`, `:1258`, `:1312`; marker: `ackStarted.promise` race, `maxActiveCalls` 1 | TE22, TE23 |
| NACK pause with a following pass | `:2968`; marker: `nackStarted.promise` race, per-identity tuples | TE22, TE23 |
| Outcome-unknown send | `:3137`, `:2223`; marker: `calls` 1 and `failureCount` 1 after the throw | TE26, TE27 |
| Reconnect after a source or wire fault | `:1870` (2); positive controls `:594` (3), `:640` | TE17, TE26 |
| Applied previous output mutates before advertisement or reuse | Hook refusal only: `:1980`. No data-value mutation witness; awaits #538 | TE21 |
| Optional-output bytes fill before session count | No witness; `wireCaches` is count-bounded (64). TE25 is #538 | TE25 (outside this supplement) |
| Changed output followed by raw append | `:2907`, `:1312` dispatch a later delta; no delta/full control | TE30 |
| Real transport abort through the lease signal | `:2119` uses an in-process client that rejects on abort; no daemon transport witness | TE24 |

## Enabling-state markers observed

These are assertions in the executed tests, not installed production markers.
Each fires on a correct implementation before the fault is introduced.

- A valid capture exists and a named await is paused: `started.promise`,
  `reachedPage1.promise`, `pageRead.promise`, `ackStarted.promise`,
  `nackStarted.promise` races against the pass; `expect(calls)` or
  `expect(bodies)` length before the fault; `expect(pageSizes).toEqual(
  [MODULE_ORDINAL_PAGE_SIZE])`; `transportProcessedBeforeInstall`.
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
   timing at `:2387` and the model-based test at
   `transform-capture.test.ts:678`.
2. A pause-and-mutate witness at the `recheckCapture("permission")` boundary.
3. TE21 and TE30 integration evidence with #538; TE25's separate byte budget.
4. Real transport abort and response-loss evidence where in-process fakes
   cannot answer the property.

# capture-charge-outlives-cancellation

## Discovery trigger

Cancellation requests do not prove that owned work or retained references have
settled. Parent #525 requires the charge to survive until actual cleanup.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner` after merging
`origin/main` at `5def3c71`. Line numbers were verified against `d5a525e8`.

- In [transform-capture.ts](../../../../../packages/opencode-plugin/src/hooks/context/transform-capture.ts),
  a lease's `requestCancel` only aborts its controller (`:553-555`);
  `release` is idempotent, subtracts the charge once, and removes the session
  entry (`:556-562`). `admit` on a held session requests cancellation of the
  holder and returns `session_busy` without taking the slot (`:521-527`).
  The owner's `requestCancel` forwards to the held lease (`:568-570`).
- In [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts),
  `clearSession` (`:1543`) and `invalidateWireState` (`:817-820`) request
  cancellation; the lease signal is passed to every transport call
  (`:1308`) and to the ordinal scan budget (`:1168`); `assertCurrentPass`
  converts an aborted signal into a `superseded` decline (`:961`) and runs
  before each page (`:1299`), after the directory and permission awaits
  (`:1087`, `:1096`), and inside every recheck (`:985`); `run` releases only
  in `.finally` after `execute` settles (`:1529`). On success the publication
  block has already transferred the retained state to `wireCaches` and
  `states` (`:1479-1486`) before that release; the transferred state is
  count-bounded and its byte budget is TE25 in #538.
- Reachability is `explicit-config-only`: the Rust-mode
  [hook](../../../../../packages/opencode-plugin/src/hooks/context/hook.ts)
  calls the owner and routes lifecycle events;
  [resolveTransformMode](../../../../../packages/opencode-plugin/src/config/transform-mode.ts)
  requires configuration and user-tier consent.

## Failure scenario

If cancellation freed the slot and charge before protected state was dropped,
a replacement could retain another large capture while the first remained
live.

## Timing windows and dependencies

The critical interval starts at the cancellation request and ends at actual
settlement of the owner's promise or accounted transfer, not at an abort
notification or a release call.

## What a test must construct

Hold an owner in preflight, ordinal work, or transport. Request cancellation
through a newer call, clear, or invalidation, then observe the held slot and
charge. Settle the owner and observe release exactly once. A repeated release
must not affect a replacement lease. Test a transport that honors the abort
separately from a fake that ignores it.

## Investigation log

### Q: Does every release follow cleanup rather than cancellation alone?

- Sources examined: lease operations, the transform lifecycle wiring, and the
  witnesses below.
- Findings: Both a signal-honoring client and a signal-ignoring client hold
  the slot and charge through cancellation and release them only when the
  owner's promise settles. Stale-lease release cannot affect a replacement.
- Missing evidence: Real daemon transport abort through the lease signal; the
  in-process clients stand in for it.
- Conclusion: resolved as exercised; the witness list is under the next
  question.

### Q: Does the lease signal reach every cancellation point before release?

- Sources examined: the lease operations at `transform-capture.ts:521-570`,
  the wiring at `rust-mode-transform.ts:817-820`, `:961`, `:1168`, `:1299`,
  `:1308`, `:1529`, `:1543`; the witnesses below.
- Findings: The lease signal reaches every transport page and the ordinal
  scan budget, and the page fence converts an aborted signal into a
  `superseded` decline before the next send. Release happens in `.finally`
  only.
- Missing evidence: Real daemon transport abort through the lease signal; the
  in-process clients stand in for it.
- Conclusion (2026-09-13, revision-bound run at `d5a525e8`, after merging
  `origin/main` at `5def3c71`, 1122 pass, 0 fail): resolved as exercised. Witnesses and
  markers:
  - `rust-mode-transform.test.ts:2231` "propagates <supersede|clear|
    invalidate> to the transport signal and waits for rejection before
    release" (3 cases). Marker: `started.promise` yields the transport's
    `AbortSignal` with `aborted` false and `chargedBytes > 0`; after the
    fault, `signal.aborted` true, `activePasses` 1, `chargedBytes > 0`; after
    `await pass`, both 0 and the host array holds `messages[0]`.
  - `:2992` "holds the charge through a slow cancellation until the cancelled
    owner settles". Marker: `chargedWhileHeld > 0`; after the declined newer
    call, `activePasses` 1 and `chargedBytes` equals `chargedWhileHeld`; the
    fake ignores the signal, so release waits for its response.
  - `:2367` "rejects <fault> at the persisted ordinal yield ..." (8 cases).
    Marker: `expect(admission.chargedBytes).toBe(heldBytes)` and
    `activePasses` 1 after supersession, clear, or invalidation at the yield.
  - `:3170` "shares default admission across factories and cancels the earlier
    session owner". Marker: default owner `chargedBytes` equals `heldBytes`
    after the cross-factory decline.
  - `transform-capture.test.ts:1046` "charges bytes against one aggregate
    budget and releases them exactly once". Marker:
    `a.lease.requestCancel("test")` leaves `chargedBytes` 100 (`:1057-1058`);
    a double release subtracts once.
  - `:998` "rejects invalid charges, isolates accounting, and cannot release a
    replacement lease". Marker: `first.lease.release()` after `next` was
    admitted leaves `chargedBytes` 1000 (`:1012`) and `activePasses` 1.
  - `:1020` "holds one lease per session, declines a newer call, and cancels
    the holder". Marker: two `session_busy` declines (`:1026`, `:1030`) while
    the holder stays.
  - `:889` model-based sequences include `cancel` and `stale` operations and
    compare counts and charges to the reference after each step.

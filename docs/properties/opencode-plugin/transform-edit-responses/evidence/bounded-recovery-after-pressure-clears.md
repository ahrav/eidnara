# bounded-recovery-after-pressure-clears

## Discovery trigger

Parent #525 requires recovery after pressure stops and owned work settles.
It does not promise finite recovery while work remains indefinitely unresolved.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner`, parent `b0023512`.

- In [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts),
  an admission decline in `run` logs and returns without touching session
  state (`:1510-1517`). Inside `execute`, a `PassDeclined` or an aborted
  lease logs at debug level and skips `markFailure` (`:1497-1499`); only
  other errors count as failures (`:1500-1503`). Declines therefore leave
  `failureCount` and `consecutiveFailures` unchanged and do not trip
  fail-open counting against the recovering session.
- In [transform-capture.ts](../../../../../packages/opencode-plugin/src/hooks/context/transform-capture.ts),
  `release` removes the lease and its charge (`:520-526`), so the next
  `admit` on that session or under the count limit succeeds (`:482-492`).
- Reachability is `explicit-config-only`: the Rust-mode
  [hook](../../../../../packages/opencode-plugin/src/hooks/context/hook.ts)
  invokes the owner again on a later call;
  [resolveTransformMode](../../../../../packages/opencode-plugin/src/config/transform-mode.ts)
  requires configuration and user-tier consent.

## Failure scenario

A stale slot, charge, or failure state prevents a later valid pass after the
original fault has stopped.

## Timing windows and dependencies

The bounded fault-free window starts after actual settlement, with enough
count and byte capacity for the next valid call. The bound is one attempt,
not a wall-clock deadline on unresolved work.

## What a test must construct

First construct pressure or a transport error and observe the declined or
failed attempt. Remove the cause, settle all relevant work, and establish
available capacity independently. Invoke `run` once and require a transform
dispatch. For byte pressure, make the next input fit the budget. Do not count
repeated polling attempts as a one-attempt recovery witness.

## Investigation log

### Q: Do recovery tests establish the fault-free settlement premise?

- Sources examined: the decline paths, lease release, and the witnesses below.
- Findings: Each witness observes settlement through `activePasses` 0 and
  `chargedBytes` 0, or through an explicit blocker release, before a single
  `run` call whose dispatch is asserted by a `calls` or `bodies` increment of
  one. None polls.
- Missing evidence: None for the decline and transport-error classes this
  revision implements. Indefinitely unresolved work is outside the premise.
- Conclusion (2026-09-13, revision-bound run, 1098 pass, 0 fail): resolved
  as exercised. Witnesses and markers:
  - `rust-mode-transform.test.ts:2756` "keeps every pass within the global
    count limit and declines without queueing". Marker: after
    `Promise.all(passes)`, `activePasses` 0, `chargedBytes` 0, and
    `failureCount` 0 for the declined session; one `run` gives `calls` 3.
  - `:2805` "declines on byte pressure alone while the aggregate charge stays
    within the budget". Marker: `chargedBytes` 0 after `await first`; one
    `run` gives `calls` 2 and `initialized` true.
  - `:2037` "recovers with a full request when byte pressure rejects a
    full-sync retry". Marker: `blocker.lease.release()`, then one `run` sends
    a body without `tail_delta`, clears `forceFullWire`, and ACKs `applied`.
  - `:2168` "charges every existing ordinal entry and ID before copying a warm
    memo". Marker: `chargedBytes` 0 after the blocker releases; one `run`
    gives `bodies` 2 with a tail delta and the memo unchanged.
  - `:2255` "rejects <fault> at the persisted ordinal yield ..." (8 cases).
    Marker: `activePasses` 0 and `chargedBytes` 0 after settlement; one `run`
    gives `calls` 1 and `entries.size` equal to `rows.length`.
  - `:3052` "shares default admission across factories and cancels the earlier
    session owner". Marker: default owner counters at 0; the second factory's
    one `run` dispatches once and sets `initialized`.
  - `:3137` "does not resend after an outcome-unknown transport failure and
    recovers on the next attempt". Marker: `calls` 2 and
    `consecutiveFailures` 0 after one recovery call.
  - `:2968` "releases rejected capture state before a paused NACK and keeps
    delivery IDs separate". Marker: counters at 0 during the paused NACK; the
    next `run` dispatches once under `maxPasses: 1`.
  - `:1112` and `:1052`: after a rejected continuation, one `run` publishes
    and ACKs.

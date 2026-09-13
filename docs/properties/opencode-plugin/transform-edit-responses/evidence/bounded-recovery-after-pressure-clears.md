# bounded-recovery-after-pressure-clears

## Discovery trigger

Parent #525 requires recovery after pressure stops and owned work settles.
It does not promise finite recovery while work remains indefinitely unresolved.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner` after merging
`origin/main` at `5def3c71`. Line numbers were verified against that tree.

- In [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts),
  an admission decline in `run` logs (warn for `pass_count`, debug for
  `session_busy`) and returns without touching session state
  (`:1515-1521`). Inside `execute`, a `PassDeclined` or an aborted lease logs
  at the decline's level and skips `markFailure` (`:1498-1503`); only other
  errors count as failures (`:1504-1507`). Declines therefore leave
  `failureCount` and `consecutiveFailures` unchanged and do not trip
  fail-open counting against the recovering session. A pass that dispatched
  and then declined leaves `forceFullWire` true (`:1361`), so its recovery
  pass sends the full history and publishes.
- In [transform-capture.ts](../../../../../packages/opencode-plugin/src/hooks/context/transform-capture.ts),
  `release` removes the lease and its charge (`:537-543`), so the next
  `admit` on that session or under the count limit succeeds (`:499-509`).
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
- Conclusion: resolved as exercised; the witness list is under the next
  question.

### Q: Does a declined dispatched pass recover in one attempt?

- Sources examined: `rust-mode-transform.ts:1361`, `:1498-1507`,
  `:1515-1521`, `transform-capture.ts:499-509`, `:537-543`; the witnesses
  below.
- Findings: A decline skips `markFailure` and the lease release frees the
  slot and charge. The source-declined witness declines a dispatched delta
  pass at publication, observes `consecutiveFailures` 0 and `forceFullWire`
  true, and shows the next single `run` sending the full history and clearing
  the flag.
- Missing evidence: None for the decline and transport-error classes this
  revision implements. Indefinitely unresolved work is outside the premise.
- Conclusion (2026-09-13, revision-bound run after merging `origin/main` at
  `5def3c71`, 1107 pass, 0 fail): resolved as exercised. Witnesses and
  markers:
  - `rust-mode-transform.test.ts:2750` "keeps every pass within the global
    count limit and declines without queueing". Marker: after
    `Promise.all(passes)`, `activePasses` 0, `chargedBytes` 0, and
    `failureCount` 0 for the declined session; one `run` gives `calls` 3.
  - `:2799` "declines on byte pressure alone while the aggregate charge stays
    within the budget". Marker: `chargedBytes` 0 after `await first`; one
    `run` gives `calls` 2 and `initialized` true.
  - `:2025` "recovers with a full request when byte pressure rejects a
    full-sync retry". Marker: `blocker.lease.release()`, then one `run` sends
    a body without `tail_delta`, clears `forceFullWire`, and ACKs `applied`.
  - `:2156` "charges every existing ordinal entry and ID before copying a warm
    memo". Marker: `chargedBytes` 0 after the blocker releases; one `run`
    gives `bodies` 2 with a tail delta and the memo unchanged.
  - `:2243` "rejects <fault> at the persisted ordinal yield ..." (8 cases).
    Marker: `activePasses` 0 and `chargedBytes` 0 after settlement; one `run`
    gives `calls` 1 and `entries.size` equal to `rows.length`.
  - `:3046` "shares default admission across factories and cancels the earlier
    session owner". Marker: default owner counters at 0; the second factory's
    one `run` dispatches once and sets `initialized`.
  - `:3131` "does not resend after an outcome-unknown transport failure and
    recovers on the next attempt". Marker: `calls` 2 and
    `consecutiveFailures` 0 after one recovery call.
  - `:2962` "releases rejected capture state before a paused NACK and keeps
    delivery IDs separate". Marker: counters at 0 during the paused NACK; the
    next `run` dispatches once under `maxPasses: 1`.
  - `:803` "forces a full send after a dispatched delta pass is
    source-declined". Marker: after the declined pass, `consecutiveFailures`
    0 and `forceFullWire` true (`:824-825`); one `run` sends `bodies[3]` with
    no `tail_delta`, `native_messages` equal to the new input, and
    `forceFullWire` false (`:830-832`).
  - `:1144` and `:1084`: after a rejected continuation, one `run` publishes
    and ACKs.

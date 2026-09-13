# owner-admission-covers-live-captures

## Discovery trigger

Parent #525 requires admission before asynchronous preflight and conservative
charges for protected pass-local state until cleanup or accounted transfer.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner`, parent `b0023512`.

- In [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts),
  `run` calls `captureAdmission.admit(sessionId)` before any await
  (`:1509-1517`); a decline logs and returns a resolved promise without
  touching state or `markFailure`. `execute` then charges the capture
  estimate (`:976`), captures (`:977`), computes the wire delta and reserves
  `WIRE_PROJECTION_FACTOR` (4, `:716`) times each shipped message's JSON
  estimate (`:996-1002`), and reserves the memo copy (`:1003-1006`), all
  before the first await at `:1018`. Ordinal annotation and scan pages charge
  through the same lease (`:1153-1165`), and candidate slots are charged at
  `CANDIDATE_SLOT_BYTES` (8) each (`:714`, `:1433`).
- The lease releases in `.finally` after `execute` settles and before
  `deliverTransformNotes` (`:1520-1522`). By then the publication block has
  transferred the candidate array, `captured.snapshots` (as the wire cache's
  `rawContentSnapshots`, `:289`), and the promoted memo to the 64-session
  `wireCaches` owner (`:126`, `:787`, `:1481`) and to `states` (`:1477`).
  That retention is count-bounded by session, not byte-bounded. The separate
  64 MiB optional-output byte budget with byte-triggered LRU eviction is TE25
  and belongs to #538. This is the current boundary, not a defect.
- In [transform-capture.ts](../../../../../packages/opencode-plugin/src/hooks/context/transform-capture.ts),
  the walk charges `TAPE_SLOT_BYTES` (8, `:9`) plus two bytes per UTF-16 unit
  of a retained string or symbol description (`:116-124`), one slot per
  descriptor read (`:192-198`), and the declared array length before element
  reads (`:240-241`), so the byte budget also bounds traversal work.
  `TransformCaptureAdmission` (`:450-535`) defaults to 64 passes and 64 MiB,
  keeps one lease per session, and rejects injected limits above the ceiling.
- Reachability is `explicit-config-only`: the Rust-mode
  [hook](../../../../../packages/opencode-plugin/src/hooks/context/hook.ts)
  (`:337-345`) calls `run` with the default owner;
  [resolveTransformMode](../../../../../packages/opencode-plugin/src/config/transform-mode.ts)
  requires configuration and user-tier consent.

## Failure scenario

A pass allocates protected state before reserving it, or releases its charge
while references remain live. Counter limits then hold while the resource
contract fails. The budget is not a whole-process RSS bound.

## Timing windows and dependencies

Unresolved preflight, ordinal scans, transport, cancellation settlement, and
delivery waits extend ownership. Successful transfer and failure cleanup need
separate accounting checks.

## What a test must construct

Inject small count and byte limits, hold admitted passes at relevant awaits,
and attempt same-session and cross-session admission. Observe charge-before-
allocation and the exact held charge as well as count/byte totals. Pause ACK
and NACK delivery to check cleanup and next-pass admission independently.

## Investigation log

### Q: Do counters account for every protected live owner through release?

- Sources examined: `run`, the charge sites, the publication block, the
  lease implementation, and the witnesses below.
- Findings: The byte-pressure witness asserts the exact held charge as the
  sum of capture, four times wire bytes, annotation, and memo charges. The
  1,000-message fixture shows the default budget admits a realistic long
  session with room for a second. The model-based test compares counts and
  charges to a reference across 200 seeded sequences.
- Missing evidence: TE25's byte budget for transferred output state.
- Conclusion (2026-09-13, revision-bound run, 1098 pass, 0 fail): resolved
  as exercised. Witnesses and markers:
  - `rust-mode-transform.test.ts:1813` "admits a full pass over 1,000
    realistic messages under the default budget with room for a second
    session". Marker: `jsonBytes > 2 * 1024 * 1024`; `peakCharge > jsonBytes`
    and `peakCharge < admission.remainingBytes / 2`; `chargedBytes` 0 after.
  - `:2805` "declines on byte pressure alone while the aggregate charge stays
    within the budget". Marker: `expect(admission.chargedBytes).toBe(
    heldCharge)` with `activePasses` 1 while the first pass is paused;
    `smallInspection.estimatedBytes > remainingBytes`; the small session
    declines with `capture_bytes` and `failureCount` 0.
  - `:2756` "keeps every pass within the global count limit and declines
    without queueing". Marker: `activePasses` 2, `calls` 2, decline log
    `pass_count`, declined array identity preserved.
  - `:2907` "releases capture admission before the ACK so a paused ACK does
    not block the next pass". Marker: `ackStarted.promise` race, then
    `activePasses` 0, `chargedBytes` 0, `remainingBytes` 64 MiB, and a second
    pass dispatches under `maxPasses: 1`.
  - `:2968` "releases rejected capture state before a paused NACK and keeps
    delivery IDs separate" (same counters at the paused NACK).
  - `:3052` "shares default admission across factories and cancels the
    earlier session owner". Marker: `defaultTransformCaptureAdmission`
    counters held across factories until the first settles.
  - `:2255` (8 cases). Marker: `heldBytes > MODULE_ORDINAL_PAGE_SIZE *
    ORDINAL_ENTRY_RETAINED_BYTES` at the scan yield.
  - `:2037`, `:2168`, `:1916`: blocker leases leave exact remainders and the
    decline logs name `wire projection`, `ordinal memo copy`, and
    `native candidate array`.
  - `transform-capture.test.ts:678` "keeps counts and charges equal to a
    reference model across generated lease sequences". Marker: each step
    asserts `[owner.activePasses, owner.chargedBytes, detail]` equals the
    model with the seed and trace in `detail`. The implementer reports a
    manual negative control: removing `owner.leases.delete` in `release`
    made this test fail. That control was not re-run for this document.
  - `:760`, `:787`, `:817`, `:833`, `:843`, `:867`, `:944`: default limits,
    invalid charges, per-session lease, count decline, exact-once release,
    reservation before capture, and exact byte charge.
  - `hook.test.ts:337` "admits before the actual hook directory await and
    never restores a rebound output". Marker: `session.get` called once; the
    second call declines `session_busy` without a directory read.

# owner-admission-covers-live-captures

## Discovery trigger

Parent #525 requires admission before asynchronous preflight and conservative
charges for protected pass-local state until cleanup or accounted transfer.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner` after merging
`origin/main` at `5def3c71`. Line numbers were verified against `d5a525e8`.

- In [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts),
  `run` calls `captureAdmission.admit(sessionId)` before any await
  (`:1517-1525`); a decline logs at warn for `pass_count` and debug for
  `session_busy` (`:1520`) and returns a resolved promise without touching
  state or `markFailure`. `execute` then charges the capture estimate
  (`:979`), captures (`:980`), computes the wire delta and reserves
  `WIRE_PROJECTION_FACTOR` (4, `:712`) times each shipped message's JSON
  estimate (`:999-1005`), and reserves the memo copy (`:1006-1009`), all
  before the first await at `:1049`. A `capture_bytes` decline logs at warn
  (`PassDeclined` default at `:735`, `:1499-1506`). Ordinal annotation and
  scan pages charge through the same lease (`:1158-1170`), and candidate
  slots are charged at `CANDIDATE_SLOT_BYTES` (8) each (`:710`, `:1444`)
  inside `buildNativeCandidate` (`:586`, `:601`, `:622`). That slot charge is
  the only bound on candidate length: `hostArrayReplacementRejection` takes
  no length argument and applies no cap.
- The lease releases in `.finally` after `execute` settles and before
  `deliverTransformNotes` (`:1529-1530`). By then the publication block has
  transferred the candidate array and `captured.snapshots` (as the wire
  cache's `rawContentSnapshots`, `:285`) to the 64-session `wireCaches`
  owner (`:122`, `:789`, `:1485`), and the promoted memo to `state.ordinals`
  in `states` (`:1481`). Only the `wireCaches` half is count-bounded.
  `states` is a plain `Map` (`:788`); `BoundedSessionMap.set` has no eviction
  callback into it and only `clearSession` deletes from it (`:1541`), so more
  than 64 distinct undeleted sessions evict wire caches while every session's
  ordinal memo stays resident. The separate 64 MiB optional-output byte budget
  with byte-triggered LRU eviction is TE25 and belongs to #538; the missing
  `states` bound is a gap this supplement records, not part of that budget.
- In [transform-capture.ts](../../../../../packages/opencode-plugin/src/hooks/context/transform-capture.ts),
  the walk charges `TAPE_SLOT_BYTES` (8, `:9`) plus two bytes per UTF-16 unit
  of a retained string or symbol description (`:151-182`), one slot per
  descriptor read (`:247-253`), and the declared array length before element
  reads (`:296-297`), so the byte budget also bounds traversal work.
  `TransformCaptureAdmission` (`:498-571`) defaults to 64 passes and 64 MiB
  and keeps one lease per session. Neither its constructor (`:501-506`) nor
  `ReferenceableWalk`'s (`:141`) validates injected limits; callers pass safe
  integers.
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
- Conclusion: resolved as exercised; the witness list is under the next
  question.

### Q: What bounds the candidate, and which limits are validated?

- Sources examined: `run` at `rust-mode-transform.ts:1517-1525`, the charge
  sites at `:979-1009`, `:1158-1170`, `:1444`, `buildNativeCandidate` at
  `:578-626`, `hostArrayReplacementRejection` at
  `transform-capture.ts:458-474`, `TransformCaptureAdmission` at `:498-571`,
  `ReferenceableWalk` at `:141`; the witnesses below.
- Findings: The candidate bound is the slot charge in `buildNativeCandidate`;
  `hostArrayReplacementRejection` has no length parameter and no cap. The
  admission and walk constructors do not validate their limits, and no test
  asserts rejection of injected limits above a ceiling; callers pass safe
  integers. A byte-budget decline through the real hook logs at warn before
  any directory read.
- Missing evidence: TE25's byte budget for transferred output state.
- Conclusion (2026-09-13, revision-bound run at `d5a525e8`, after merging
  `origin/main` at `5def3c71`, 1122 pass, 0 fail): resolved as exercised. Witnesses and
  markers:
  - `rust-mode-transform.test.ts:1953` "admits a full pass over 1,000
    realistic messages under the default budget with room for a second
    session". Marker: `jsonBytes > 2 * 1024 * 1024` (`:1984`);
    `peakCharge < admission.remainingBytes / 2` (`:2005`); `chargedBytes` 0
    after.
  - `:2923` "declines on byte pressure alone while the aggregate charge stays
    within the budget". Marker: `expect(admission.chargedBytes).toBe(
    heldCharge)` with `activePasses` 1 while the first pass is paused;
    `smallInspection.estimatedBytes > remainingBytes`; the small session
    declines with `capture_bytes` and `failureCount` 0.
  - `:2874` "keeps every pass within the global count limit and declines
    without queueing". Marker: `activePasses` 2, `calls` 2, decline log
    `pass_count`, declined array identity preserved.
  - `:3025` "releases capture admission before the ACK so a paused ACK does
    not block the next pass". Marker: `ackStarted.promise` race, then
    `activePasses` 0, `chargedBytes` 0, `remainingBytes` 64 MiB, and a second
    pass dispatches under `maxPasses: 1`.
  - `:3086` "releases rejected capture state before a paused NACK and keeps
    delivery IDs separate" (same counters at the paused NACK).
  - `:3170` "shares default admission across factories and cancels the
    earlier session owner". Marker: `defaultTransformCaptureAdmission`
    counters held across factories until the first settles.
  - `:2367` (8 cases). Marker: `heldBytes > MODULE_ORDINAL_PAGE_SIZE *
    ORDINAL_ENTRY_RETAINED_BYTES` at the scan yield (`:2406`).
  - `:2149`, `:2280`, `:2085`: blocker leases leave exact remainders
    (`remainingBytes - candidateLength * 8 + offset` at `:2116`) and the
    decline logs name `wire projection`, `ordinal memo copy`, and
    `native candidate array`.
  - `transform-capture.test.ts:889` "keeps counts and charges equal to a
    reference model across generated lease sequences". Marker: 200 seeded
    sequences of 40 steps; each step asserts
    `[owner.activePasses, owner.chargedBytes, detail]` equals the model with
    the seed and trace in `detail`. The implementer's manual negative control
    (removing `owner.leases.delete` at `transform-capture.ts:561`) is recorded
    as reported, not re-run.
  - `transform-capture.test.ts:971`, `:998`, `:1020`, `:1036`, `:1046`,
    `:1089`, `:1166`: default limits, invalid charges, per-session lease,
    count decline, exact-once release, reservation before capture, and exact
    byte charge.
  - `hook.test.ts:301` "logs a byte-budget decline at warn before preflight".
    Marker: forty 2 MiB strings; `sessionLog.warn` receives a message
    containing `pass declined: capture_bytes` for the session;
    `client.session.get` not called; `fake.calls` 0.
  - `hook.test.ts:396` "admits before the actual hook directory await and
    never restores a rebound output". Marker: `session.get` called once; the
    second call declines `session_busy` at debug without a directory read.

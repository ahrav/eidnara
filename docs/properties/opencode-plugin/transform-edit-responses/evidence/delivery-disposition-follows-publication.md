# delivery-disposition-follows-publication

## Discovery trigger

Parent #525 binds delivery approval to publication. Rejected attempts cannot
approve their notes, and ACK failure cannot undo applied output.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner` after merging
`origin/main` at `5def3c71`. Line numbers were verified against `d5a525e8`.

- In [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts),
  `DeliveryPlan` (`:742-747`) carries `attempted` and `applied` sets. Every
  page response adds its `note_deliveries` IDs to `attempted` (`:1321`),
  including every page of a series whose source changed between pages, since
  pages are preceded by `assertCurrentPass()` only (`:1299`) and the series
  completes. `deliveries.applied` is assigned only inside the synchronous
  publication block (`:1486`). `run` releases the lease in `.finally` and then
  calls `deliverTransformNotes` (`:1529-1530`), which sends NACK for attempted
  IDs not in `applied` and ACK for the rest, sequentially, and logs failures
  as an `AggregateError` without touching output (`:749-777`). Delivery
  therefore receives only route and ID metadata, after protected state is
  dropped or transferred.
- Reachability is `explicit-config-only`: the Rust-mode
  [hook](../../../../../packages/opencode-plugin/src/hooks/context/hook.ts)
  (`:337-345`) calls `run`;
  [resolveTransformMode](../../../../../packages/opencode-plugin/src/config/transform-mode.ts)
  requires configuration and user-tier consent.

## Failure scenario

An unapplied attempt ACKs its delivery IDs, suppressing a note that the model
never received. Conversely, an ACK transport error after publication triggers
rollback.

## Timing windows and dependencies

Response receipt, each retry or rejection, publication, and delivery awaits. A
later pass can begin while an earlier applied pass awaits its ACK, because the
lease is already released. A paged series whose source changed between pages
reports deliveries on every page and is refused at publication; all of them
are NACKed.

## What a test must construct

Assign distinct delivery IDs to attempts. Pause a response, supersede or
invalidate its owner, and check that its IDs receive no ACK attempt. Require
NACK attempts for known discarded IDs, including IDs reported by pages that
completed before a publication refusal. Pause and fail an ACK after successful
publication, then verify output and promoted state remain applied.

Record attempted and transport-confirmed dispositions separately. A call to a
fake client proves neither daemon receipt nor successful note-state mutation.
Use per-identity assertions; aggregate counts can hide swapped dispositions.

## Investigation log

### Q: Are all attempt identities dispositioned according to publication?

- Sources examined: `DeliveryPlan`, `deliverTransformNotes`, the publication
  block, and the witnesses below.
- Findings: Every rejection path exercised by the ownership tests ends in a
  NACK of the attempt's known IDs; applied attempts ACK only their own IDs;
  the per-identity witness at `rust-mode-transform.test.ts:3086` asserts
  `[method, transform_pass_id]` tuples across a rejected and an accepted
  attempt (`:3143-3153`).
- Missing evidence: Daemon-side receipt is not observed; the fake client
  records attempted dispositions only.
- Conclusion: resolved as exercised; the witness list is under the next
  question.

### Q: Are deliveries from a completed but refused series NACKed?

- Sources examined: `rust-mode-transform.ts:1299`, `:1321`, `:1486`,
  `:749-777`, `:1529-1530`; the witnesses below.
- Findings: Pages are ownership fences, not rechecks, so a series whose
  source changed between pages reports deliveries on every page, completes,
  and is refused at publication; the between-pages witness asserts that the
  NACK list equals every ID the fake reported and that no ACK occurs. The
  reconnect witness's mutation case restarts the series once and NACKs both
  series' IDs.
- Missing evidence: Daemon-side receipt is not observed; the fake client
  records attempted dispositions only.
- Conclusion (2026-09-13, revision-bound run at `d5a525e8`, after merging
  `origin/main` at `5def3c71`, 1122 pass, 0 fail): resolved as exercised. Witnesses and
  markers:
  - `rust-mode-transform.test.ts:3086` "releases rejected capture state before
    a paused NACK and keeps delivery IDs separate". Marker:
    `nackStarted.promise` race after a host edit; non-transform calls equal
    `[["transform.nack", "discarded"], ["transform.ack", "accepted"]]`.
  - `:3025` "releases capture admission before the ACK so a paused ACK does
    not block the next pass". Marker: `ackStarted.promise` race with
    `firstOutput.messages[0]` already `applied[0]`; after the ACK throws,
    the same identity holds and `failureCount` is 0.
  - `:1463` "nacks discarded delivery IDs and acks only IDs from the applied
    retry response" (NACK `pass-initial`; ACK `pass-shared`, `pass-retry`).
  - `:1505` "nacks initial and retry delivery IDs when the full retry still
    cannot be applied" (no ACK call at all).
  - `:1393` "disposes each duplicate delivery pass ID only once".
  - `:1290` "attempts every ack sequentially before reporting aggregate
    failures" (`maxActiveCalls` 1; `AggregateError` with the one failure).
  - `:1417` "attempts every nack without replacing the original apply error".
  - `:1535` "nacks note deliveries and serves the input unchanged when the
    boundary lacks a synthetic m0".
  - `:1242` "applies a native_messages_delta in place and acks its note
    deliveries" (exact ACK body for `pass-1`).
  - `:941` "nacks note deliveries when a pass is superseded while its
    transform response is pending" (`calls` 1 before the newer call).
  - `:2660` "declines before publication and NACKs known deliveries when the
    source changes between pages". Marker: the fake records each
    `transform_pass_id` it reports in `delivered`; after the pass, no
    `transform.ack` call exists and the `transform.nack` IDs equal
    `delivered` (`:2688-2694`).
  - `:2010` "stops a series restart after <reconnect|attempt-mismatch> when
    <mutation|accessor|invalidation> lands first" (6 cases). Marker: no ACK;
    NACK IDs sorted equal `["page-zero"]` for every fault because the
    `series-restart` recheck refuses the restart (`:2071-2078`).
  - `:2594` (4 cases; NACK of `retry-discarded`), `:2085` (2 cases; ACK of
    `candidate` on publish, NACK on decline), `:2149` (NACK `discarded`, then
    ACK `applied` on the recovery pass), `:2454` (8 cases; NACK of
    `unapplied`), `:2499` (pre-apply cases; NACK of `nested-unapplied`).

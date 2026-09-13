# uncertain-send-never-replays-blindly

## Discovery trigger

Parent #525 forbids blind replay when transport failure leaves the daemon-side
outcome unknown. Named application recovery is a separate contract.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner`, parent `b0023512`.

- In [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts),
  `sendTransformSeries` rethrows any transport error that is not a classified
  page attempt mismatch (`:1307-1311`); only a reconnect result or an attempt
  mismatch returns a restart marker (`:1312-1315`).
  `sendTransformSeriesWithSingleRestart` permits one restart per pass and
  throws on a second (`:1333-1356`). A thrown error reaches the catch at
  `:1490`, which records a failure through `markFailure` (`:1502`) and never
  re-sends. The full-array retry after `need_full_sync` (`:1359-1420`) is the
  named application recovery and is itself sent once.
- Reachability is `explicit-config-only`: the Rust-mode
  [hook](../../../../../packages/opencode-plugin/src/hooks/context/hook.ts)
  calls the transform series path;
  [resolveTransformMode](../../../../../packages/opencode-plugin/src/config/transform-mode.ts)
  requires configuration and user-tier consent.

## Failure scenario

A generic retry repeats a transform already accepted by the daemon. The client
must not infer non-application from a missing response.

## Timing windows and dependencies

The target window is after request transmission but before a usable response.
The fake-client witnesses construct a thrown call, not the wire window itself.

## What a test must construct

Retain the one-call negative control for generic errors. Keep bounded page
restart and full-sync recovery positive controls so the test does not pass by
disabling all legitimate recovery. To establish the real unknown-outcome
window, observe request acceptance, lose its response, and assert no automatic
resend, recording attempted and acknowledged effects by identity.

## Investigation log

### Q: Does real response loss avoid blind replay?

- Sources examined: series error classification, the single-restart wrapper,
  the catch path, and the witnesses below.
- Findings: A thrown first send records one `transform` call and one failure;
  a thrown full retry records no further send; a reconnect after a source or
  wire fault stops the bounded restart; the positive controls show exactly one
  restart on a classified mismatch or reconnect.
- Missing evidence: A real transport that writes the request and loses the
  response; the fake throws before returning, so daemon-side application of
  the lost request is not observable here.
- Conclusion (2026-09-13, revision-bound run, 1098 pass, 0 fail): resolved
  as exercised for the generic-error contract. Witnesses and markers:
  - `rust-mode-transform.test.ts:3137` "does not resend after an
    outcome-unknown transport failure and recovers on the next attempt".
    Marker: `expect(calls).toHaveLength(1)`, `failureCount` 1, and the host
    array unchanged after the throw; the next call gives `calls` 2.
  - `:2223` "retains full-sync recovery after a failed retry until a full
    request publishes". Marker: `bodies` 3 after the thrown full retry
    (warm send, tail delta, thrown full retry) and `forceFullWire` true; the
    next call sends one full body and clears the flag.
  - `:1870` "stops a series restart after a mid-series reconnect when
    <mutation|invalidation> lands first" (2 cases). Marker: exactly one
    page-zero body before and after the reconnect result, and
    `["transform", "transform", "transform.nack"]`.
  - Positive controls: `:594` "restarts a paged transform series after a
    <thrown-code|returned-code|message> attempt mismatch" (3 cases; two
    series starts with distinct page IDs) and `:640` "restarts a paged
    transform series after a mid-series reconnect" (two series starts; the
    continuation call is `generationSensitive`).

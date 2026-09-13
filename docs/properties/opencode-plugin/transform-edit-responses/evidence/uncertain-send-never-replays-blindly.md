# uncertain-send-never-replays-blindly

## Discovery trigger

Parent #525 forbids blind replay when transport failure leaves the daemon-side
outcome unknown. Named application recovery is a separate contract.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner` after merging
`origin/main` at `5def3c71`. Line numbers were verified against that tree.

- In [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts),
  `sendTransformSeries` (`:1281-1333`) rethrows any transport error that is
  not a classified page attempt mismatch (`:1310-1314`); only a reconnect
  result or an attempt mismatch returns a restart marker (`:1315-1318`).
  `sendTransformSeriesWithSingleRestart` permits one restart per pass and
  throws on a second (`:1336-1359`). A thrown error reaches the catch at
  `:1491`, which records a failure through `markFailure` (`:1506`) and never
  re-sends. The full-array retry after `need_full_sync` (`:1364-1428`) is the
  named application recovery and is itself sent once. `state.forceFullWire =
  true` precedes the first send (`:1361`), so a pass that dispatched and then
  failed or declined sends the full history next time rather than a delta
  against a snapshot the daemon may have committed.
- Each page is preceded by `assertCurrentPass()` only (`:1298`). A content
  change before a mid-series reconnect therefore does not stop the single
  restart; the restart runs over the same frozen pages and publication
  refuses it. Invalidation, clear, and supersession do stop the restart at
  the next page.
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
  a thrown full retry records no further send; a reconnect after a wire
  fault stops the bounded restart at the next page, and a reconnect after a
  source change lets the single restart run and is refused at publication;
  the positive controls show exactly one restart on a classified mismatch or
  reconnect.
- Missing evidence: A real transport that writes the request and loses the
  response; the fake throws before returning, so daemon-side application of
  the lost request is not observable here.
- Conclusion: resolved as exercised for the generic-error contract; the
  witness list is under the next question.

### Q: Which faults stop the bounded restart?

- Sources examined: `rust-mode-transform.ts:1281-1359`, `:1361`, `:1491`,
  `:1506`; the witnesses below.
- Findings: Generic errors are rethrown and the restart wrapper permits one
  restart. In the reconnect witness's mutation case the restart runs (two
  page-zero bodies) because pages are ownership fences only, and publication
  refuses the restarted series; the invalidation case stops the restart at
  the fence (one page-zero body). Neither case ACKs. The forced full send
  after a dispatched pass is set before the first send, not only on
  `need_full_sync`.
- Missing evidence: A real transport that writes the request and loses the
  response; the fake throws before returning.
- Conclusion (2026-09-13, revision-bound run after merging `origin/main` at
  `5def3c71`, 1107 pass, 0 fail): resolved as exercised for the generic-error
  contract. Witnesses and markers:
  - `rust-mode-transform.test.ts:3131` "does not resend after an
    outcome-unknown transport failure and recovers on the next attempt".
    Marker: `expect(calls).toHaveLength(1)`, `failureCount` 1, and the host
    array unchanged after the throw; the next call gives `calls` 2.
  - `:2211` "retains full-sync recovery after a failed retry until a full
    request publishes". Marker: `bodies` 3 after the thrown full retry and
    `forceFullWire` true; the next call sends one full body and clears the
    flag.
  - `:1902` "stops a series restart after a mid-series reconnect when
    <mutation|invalidation> lands first" (2 cases). Marker: exactly one
    page-zero body and `transform_page_index` 1 pending before the reconnect
    result (`:1931-1932`); after it, two page-zero bodies for mutation and
    one for invalidation (`:1942-1944`), no `transform.ack`, NACKs
    `["page-zero", "restarted"]` or `["page-zero"]`, `failureCount` 0, default
    owner `chargedBytes` 0.
  - Positive controls: `:594` "restarts a paged transform series after a
    <thrown-code|returned-code|message> attempt mismatch" (3 cases; two
    series starts with distinct page IDs) and `:640` "restarts a paged
    transform series after a mid-series reconnect" (two series starts).

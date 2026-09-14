# uncertain-send-never-replays-blindly

## Discovery trigger

Parent #525 forbids blind replay when transport failure leaves the daemon-side
outcome unknown. Named application recovery is a separate contract.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner` after merging
`origin/main` at `5def3c71`. Line numbers were verified against `d5a525e8`.

- In [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts),
  `sendTransformSeries` (`:1282-1334`) rethrows any transport error that is
  not a classified page attempt mismatch (`:1311-1315`); only a reconnect
  result or an attempt mismatch returns a restart marker (`:1316-1319`).
  `sendTransformSeriesWithSingleRestart` permits one restart per pass and
  throws on a second (`:1337-1361`). A thrown error reaches the catch at
  `:1494`, which records a failure through `markFailure` (`:1509`) and never
  re-sends. The full-array retry after `need_full_sync` (`:1366-1431`) is the
  named application recovery and is itself sent once. `state.forceFullWire =
  true` precedes the first send (`:1363`), so a pass that dispatched and then
  failed or declined sends the full history next time rather than a delta
  against a snapshot the daemon may have committed.
- Each page is preceded by `assertCurrentPass()` only (`:1299`), so a
  content change between pages of one series lets the series complete and
  is refused at publication. A restart does not continue the frozen pages: it
  calls `sendTransformSeries` again, which rebuilds the series from the
  payload (`:1286`), so `recheckCapture("series-restart")` (`:1353`) runs
  first and a content change or accessor installed before the restart refuses
  it. Invalidation, clear, and supersession stop the restart at the fence.
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

- Sources examined: `rust-mode-transform.ts:1282-1361`, `:1363`, `:1494`,
  `:1509`; the witnesses below.
- Findings: Generic errors are rethrown and the restart wrapper permits one
  restart, preceded by `recheckCapture("series-restart")` because the restart
  rebuilds the series rather than continuing frozen pages. In the restart
  witness, mutation and accessor cases are refused by that recheck and the
  invalidation case by the fence; every case sends one page-zero body,
  invokes no getter, and NACKs `["page-zero"]`. No case ACKs. The forced full send
  after a dispatched pass is set before the first send, not only on
  `need_full_sync`.
- Missing evidence: A real transport that writes the request and loses the
  response; the fake throws before returning.
- Conclusion (2026-09-13, revision-bound run at `d5a525e8`, after merging
  `origin/main` at `5def3c71`, 1122 pass, 0 fail): resolved as exercised for the generic-error
  contract. Witnesses and markers:
  - `rust-mode-transform.test.ts:3255` "does not resend after an
    outcome-unknown transport failure and recovers on the next attempt".
    Marker: `expect(calls).toHaveLength(1)`, `failureCount` 1, and the host
    array unchanged after the throw; the next call gives `calls` 2.
  - `:2335` "retains full-sync recovery after a failed retry until a full
    request publishes". Marker: `bodies` 3 after the thrown full retry and
    `forceFullWire` true; the next call sends one full body and clears the
    flag.
  - `:2010` "stops a series restart after <reconnect|attempt-mismatch> when
    <mutation|accessor|invalidation> lands first" (6 cases). Marker: exactly
    one page-zero body and `transform_page_index` 1 pending before the restart
    result (`:2047-2048`); after it, still one page-zero body and `getterCalls`
    0 (`:2067-2068`), no `transform.ack`, NACK `["page-zero"]`, `failureCount`
    0, default owner `chargedBytes` 0 (`:2071-2080`).
  - Positive controls: `:594` "restarts a paged transform series after a
    <thrown-code|returned-code|message> attempt mismatch" (3 cases; two
    series starts with distinct page IDs) and `:640` "restarts a paged
    transform series after a mid-series reconnect" (two series starts).

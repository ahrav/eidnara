# captured-input-stays-coherent

## Discovery trigger

Parent #525 requires one coherent input capture across projection, fingerprinting,
paging, and publication. An await permits the host to change referenced values.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner` after merging
`origin/main` at `5def3c71`. Line numbers below are from `d5a525e8` and were
verified before writing.

- `execute` in
  [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts)
  inspects the source (`:969`), charges and captures it (`:979-980`), and then
  reads only `captured.members` (`:982`). `recheckCapture` (`:984-992`) asserts
  the current pass, that `output.messages` is still the captured target, and
  `capturedMessagesUnchanged`. It runs after each ordinal prime (`:1172`,
  phase `ordinal:<detail>`), before the wire is built (`:1215`,
  `wire-build`), before a series restart (`:1353`, `series-restart`), before
  a full-sync retry rebuilds the wire (`:1395`, `retry-wire-build`) or
  re-serializes an unchanged body (`:1421`, `full-retry`), and at
  publication (`:1471`, `publish`).
- The directory await (`:1049`) and the permission await (`:1090`) are
  followed by `assertCurrentPass()` only (`:1087`, `:1096`). Every message
  read (availability verdicts, active agent, model, synthetic turn, staged
  memo, provisional base) happens synchronously before the first await
  (`:1010-1047`; the comment at `:1047` states the contract).
- Each transport page is preceded by `assertCurrentPass()` only (`:1299`).
  Page bodies are frozen text, so a source walk there cannot change what is
  sent (`:1298`); the publish recheck covers the series. A content change
  between pages therefore does not stop the series early: it is refused at
  publication and every reported delivery is NACKed. Invalidation, clear, and
  supersession still stop the series at the next page (`assertCurrentPass` at
  `:957-962`).
- `state.forceFullWire = true` is set right before the first daemon send
  (`:1363`), after the page fence, because the daemon commits its snapshot on
  response; a dispatched pass that later declines forces the next pass to send
  the full history. Publication clears it (`:1484`).
- `capturedMessagesUnchanged` in
  [transform-capture.ts](../../../../../packages/opencode-plugin/src/hooks/context/transform-capture.ts)
  (`:422-449`) compares membership and content tapes without reading
  accessors; a `SourceRejected` from the built-in prototype scan
  (`rootArrayRejection`, `:94-117`) also reads as changed.
- The hook-side `messagesTransform` in
  [hook.ts](../../../../../packages/opencode-plugin/src/hooks/context/hook.ts)
  (`:337-345`) reads `output.messages` through `readOwnDataProperty` and calls
  `rustTransform.run(sessionId, output)`; `run` has no separate `messages`
  argument (`rust-mode-transform.ts:1516`).
- Reachability is `explicit-config-only`: `hook.ts:337` builds the Rust-mode
  transform only when `rustMode` is true, and
  [resolveTransformMode](../../../../../packages/opencode-plugin/src/config/transform-mode.ts)
  (`:18`) requires Rust configuration and user-tier consent.

## Failure scenario

The host edits `parts[0].text` while a request is pending. Publishing a response
for the old text would apply stale output.

## Timing windows and dependencies

Boundaries: directory resolution, permission lookup, ordinal scans, paging,
the full-sync retry, series restart, and response application. Ordinal
scans, the full-sync retry, and application are guarded by a recheck that
reads only descriptors and captured tapes. Directory, permission, and each
page are ownership fences; a content change there is caught by the next
recheck, at the latest at publication.

## What a test must construct

Pause a relevant await after a valid capture, mutate membership or nested
content, then resume. Compare the host array with its current mutated contents,
not a stale pre-capture copy. Assert no publication and no ACK, and a NACK for
every delivery the daemon reported. An earlier valid dispatch is allowed; a
series may complete before the refusal. Check known delivery IDs, shared memo
state, and `forceFullWire` separately. Repeat at each boundary rather than
inferring coverage from one transport pause.

## Investigation log

### Q: Does every relevant await preserve coherent source use?

- Sources examined: `execute`, `recheckCapture`, capture helpers, and the
  witnesses in `rust-mode-transform.test.ts` and `hook.test.ts`.
- Findings: One mechanism covers every await. `recheckCapture` runs after
  each ordinal prime, before the wire is built, before a series restart,
  before the full-sync retry, and at publication;
  `assertCurrentPass()` fences the directory await, the permission await, and
  each transport page. Witnesses pause the directory await, the ordinal
  yield, the transport pages, the full-sync retry, and the pre-apply
  microtask, and mutate before resuming.
- Missing evidence: A clear, invalidation, or supersession witness landed
  during the directory or permission await; see the next question.
- Conclusion: resolved as exercised; the witness list is under the next
  question.

### Q: Where are content changes caught, even between pages?

- Sources examined: `execute` at `rust-mode-transform.ts:984-992`,
  `:1047-1049`, `:1087`, `:1096`, `:1172`, `:1298-1299`, `:1363`, `:1421`,
  `:1471`; `capturedMessagesUnchanged` at `transform-capture.ts:422-449`; the
  witnesses below.
- Findings: The recheck runs at six points: after each ordinal prime,
  before the wire is built, before a series restart, before the full-sync
  retry rebuilds or re-serializes its body, and at publication. Directory,
  permission, and each page are ownership fences. A content change between
  pages lets the series complete and is refused at publication with every
  reported delivery NACKed; a mutation or accessor before a mid-series
  reconnect or attempt mismatch is refused by the `series-restart` recheck
  before the restart is sent. A dispatched pass that later declines leaves `forceFullWire` true so
  the next pass sends the full history.
- Missing evidence: A clear, invalidation, or supersession witness landed
  during the directory or permission await specifically; those fences are
  exercised at the page fence and the ordinal yield only.
- Conclusion (2026-09-13, revision-bound run at `d5a525e8`, after merging
  `origin/main` at `5def3c71`, 1122 pass, 0 fail): resolved as exercised. Witnesses and their
  enabling-state markers:
  - `rust-mode-transform.test.ts:2732` "permits no further dispatch or
    publication when an accessor is installed during preflight". Marker:
    `started.promise` races the pass on `session.get`; after the getter
    install on `info`, `expect(getter).not.toHaveBeenCalled()`, `calls` 0,
    `output.messages` is `array`. The change is caught at the
    `ordinal:attempt=first` recheck.
  - `:2770` "rejects publication when a message is edited in place while the
    transform response is pending". Marker: `calls` 1 before the edit; then
    `["transform", "transform.nack"]`.
  - `:2367` "rejects <fault> at the persisted ordinal yield with <shared|
    distinct> arrays" (8 cases). Marker: `pageSizes` equals
    `[MODULE_ORDINAL_PAGE_SIZE]` and `calls` 0 at the yield.
  - `:2454` "preserves a host <member|append|rebind|metadata> during transport
    with <shared|distinct> source" (8 cases). Marker: `calls` 1 before the
    change; `["transform", "transform.nack"]` after, `entries.size` 0.
  - `:2499` "rejects nested <accessor|toJSON|proxy> installed at <source-await|
    pre-apply> without invoking it" (6 cases). Markers: `directoryReached`
    true with `methods` empty, or `transportProcessedBeforeInstall` true.
  - `:2594` "does not dispatch a need_full_sync retry after <fault> of the
    valid first send" (4 cases). Marker: `bodies` 2 with `tail_delta` defined
    before the fault; `bodies` stays 2 and `forceFullWire` equals
    `fault !== "clear"` (`:2655`).
  - `:2660` "declines before publication and NACKs known deliveries when the
    source changes between pages". Marker: the fake installs the `parts`
    accessor when it receives `transform_page_index` 1; `bodies.length > 1`
    and the last body carries `transform_page_complete` true, so the series
    completed; `expect(hook).not.toHaveBeenCalled()`, `output.messages[0]` is
    `member`, no `transform.ack`, the NACK IDs equal `delivered`,
    `entries.size` 0, `failureCount` 0.
  - `:2010` "stops a series restart after <reconnect|attempt-mismatch> when
    <mutation|accessor|invalidation> lands first" (6 cases). Marker: one
    page-zero body and `transform_page_index` 1 pending before the fault
    (`:2047-2048`). After the restart result, still one page-zero body: the
    `series-restart` recheck refuses the restart for every fault, `getterCalls`
    0, NACK `["page-zero"]` (`:2067-2078`).
  - `:803` "forces a full send after a dispatched delta pass is
    source-declined". Marker: the fake replaces `live[0]` while producing the
    response to a `tail_delta` body; `live[0]` is not `original`, `calls`
    holds 3 `transform` entries, `consecutiveFailures` 0, `forceFullWire`
    true; the next run's `bodies[3]` has no `tail_delta` and clears the flag.
  - `hook.test.ts:360` "rejects source mutation during hook directory lookup
    before direct transform". Marker: `client.session.get` called once before
    the `parts` getter install; `fake.calls` 0 after and the getter descriptor
    is still in place.

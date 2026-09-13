# captured-input-stays-coherent

## Discovery trigger

Parent #525 requires one coherent input capture across projection, fingerprinting,
paging, and publication. An await permits the host to change referenced values.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner`, parent `b0023512`. Line
numbers below are from that tree and were verified before writing.

- `execute` in
  [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts)
  inspects the source (`:967`), charges and captures it (`:976-977`), and then
  reads only `captured.members` (`:979`). `recheckCapture` (`:981-989`) asserts
  the current pass, that `output.messages` is still the captured target, and
  `capturedMessagesUnchanged`. It runs at `preflight` (`:1056`), `permission`
  (`:1071`), each ordinal scan (`:1167`), each transport page (`:1295`), and
  `publish` (`:1467`).
- `capturedMessagesUnchanged` in
  [transform-capture.ts](../../../../../packages/opencode-plugin/src/hooks/context/transform-capture.ts)
  (`:364`) compares membership and content tapes without reading accessors.
- The hook-side `messagesTransform` in
  [hook.ts](../../../../../packages/opencode-plugin/src/hooks/context/hook.ts)
  (`:337-345`) reads `output.messages` through `readOwnDataProperty` and calls
  `rustTransform.run(sessionId, output)`; `run` has no separate `messages`
  argument (`rust-mode-transform.ts:1509`).
- Reachability is `explicit-config-only`: `hook.ts:337` builds the Rust-mode
  transform only when `rustMode` is true, and
  [resolveTransformMode](../../../../../packages/opencode-plugin/src/config/transform-mode.ts)
  (`:18`) requires Rust configuration and user-tier consent.

## Failure scenario

The host edits `parts[0].text` while a request is pending. Publishing a response
for the old text would apply stale output.

## Timing windows and dependencies

Boundaries: directory resolution, permission lookup, ordinal scans, paging,
the full-sync retry, series restart, and response application. Each is guarded
by a recheck that reads only descriptors and captured tapes.

## What a test must construct

Pause a relevant await after a valid capture, mutate membership or nested
content, then resume. Compare the host array with its current mutated contents,
not a stale pre-capture copy. Assert no further dispatch, publication, or ACK.
An earlier valid dispatch is allowed. Check known delivery IDs and shared memo
state separately. Repeat at each boundary rather than inferring coverage from
one transport pause.

## Investigation log

### Q: Does every relevant await preserve coherent source use?

- Sources examined: `execute`, `recheckCapture`, capture helpers, and the
  witnesses in `rust-mode-transform.test.ts` and `hook.test.ts`.
- Findings: The mechanism is one function applied at every await listed above.
  Witnesses pause each await except `permission` and mutate before resuming.
- Missing evidence: A dedicated pause-and-mutate witness at the
  `recheckCapture("permission")` boundary (`:1071`).
- Conclusion (2026-09-13, revision-bound run, 1098 pass, 0 fail): resolved
  as exercised. Witnesses and their enabling-state markers:
  - `rust-mode-transform.test.ts:2614` "permits no further dispatch or
    publication when an accessor is installed during preflight". Marker:
    `started.promise` races the pass on `session.get`; after the getter
    install, `expect(getter).not.toHaveBeenCalled()` and `calls` 0.
  - `:2652` "rejects publication when a message is edited in place while the
    transform response is pending". Marker: `expect(calls).toHaveLength(1)`
    before the edit; then `["transform", "transform.nack"]`, `output.messages`
    is the original array holding `member`.
  - `:2255` "rejects <fault> at the persisted ordinal yield with <shared|
    distinct> arrays" (8 cases). Marker:
    `expect(pageSizes).toEqual([MODULE_ORDINAL_PAGE_SIZE])` and `calls` 0 at
    the yield; afterwards `calls` 0, `entries.size` 0, `initialized` false.
  - `:2342` "preserves a host <member|append|rebind|metadata> during transport
    with <shared|distinct> source" (8 cases). Marker: `calls` 1 before the
    change; `output.messages` is `currentArray` after, with NACK of
    `unapplied`.
  - `:2387` "rejects nested <accessor|toJSON|proxy> installed at <source-await|
    pre-apply> without invoking it" (6 cases). Markers: `directoryReached`
    true with `methods` empty, or `transportProcessedBeforeInstall` true.
  - `:2482` "does not dispatch a need_full_sync retry after <fault> of the
    valid first send" (4 cases). Marker: `expect(bodies).toHaveLength(2)` and
    `bodies[1].tail_delta` defined before the fault; `bodies` stays 2.
  - `:2548` "stops a multi-page send after page zero when its source
    changes". Marker: `transform_page_index` 0, `transform_page_total > 1`,
    `transform_page_complete` false before the edit; one body after.
  - `:1870` "stops a series restart after a mid-series reconnect when
    <mutation|invalidation> lands first" (2 cases). Marker: exactly one
    page-zero body and `transform_page_index` 1 pending.
  - `hook.test.ts:301` "rejects source mutation during hook directory lookup
    before direct transform". Marker: `client.session.get` called once before
    the getter install; `fake.calls` 0 after.

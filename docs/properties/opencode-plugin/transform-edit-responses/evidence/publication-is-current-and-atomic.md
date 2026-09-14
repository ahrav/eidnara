# publication-is-current-and-atomic

## Discovery trigger

Parent #525 requires complete validation before host writes, current ownership
at publication, and preservation of OpenCode's array identity on every outcome.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner` after merging
`origin/main` at `5def3c71`. Line numbers were verified against `d5a525e8`.

- [transform-capture.ts](../../../../../packages/opencode-plugin/src/hooks/context/transform-capture.ts):
  `hostArrayReplacementRejection(target)` (`:458-474`) takes no length
  argument and applies no candidate-length cap. It rejects proxies,
  non-arrays, non-extensible arrays, a non-writable `length`, any slot that is
  not writable and configurable, and every `SourceRejected` reason the walk
  raises (including `prototype_accessor` from `rootArrayRejection`, `:94-117`),
  walking the destination's own descriptors only. `replaceHostArrayContents`
  (`:480-485`) is a define loop over own slots plus one `length` define; its
  only precondition (`:476-479`) is that the rejection check returned `null`.
- [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts):
  `buildNativeCandidate` reserves candidate slots at `CANDIDATE_SLOT_BYTES`
  (`:1436-1445`; definition `:578-626`), `assertNativeBoundary` follows
  directly (`:1446-1449`) and reads the candidate's head entries plainly
  (`:377-383`), the continuation shift runs on the staged memo
  (`:1450-1469`), `recheckCapture("publish")` and the container check run
  (`:1471-1475`), and then replacement, `nativeOutput`, `state.ordinals`,
  `state.initialized`, `wireCaches.set`, and `deliveries.applied` are assigned
  with no await between them (`:1479-1486`). The candidate is not inspected
  before `assertNativeBoundary`; kept-prefix validation is #538's TE21.
  Successful publication transfers the candidate array and
  `captured.snapshots` (through the pending wire cache's
  `rawContentSnapshots`, `:285`) to the 64-session `wireCaches` owner
  (`:122`, `:789`) and the promoted memo to `state.ordinals` in `states`
  (`:788`, unbounded; only `clearSession` deletes, `:1541`); the lease
  releases in `.finally` after that block (`:1529`). The wire-cache half is
  count-bounded; the separate optional-output byte budget is TE25 in #538.
- [messages-transform.ts](../../../../../packages/opencode-plugin/src/plugin/messages-transform.ts)
  catches hook errors, logs them, and returns the current `output.messages`
  after a root-only `rootArrayRejection` check (`:65-82`); it never assigns
  `output.messages`.
- Reachability is `explicit-config-only`: the Rust-mode
  [hook](../../../../../packages/opencode-plugin/src/hooks/context/hook.ts)
  (`:337-345`) calls this application path;
  [resolveTransformMode](../../../../../packages/opencode-plugin/src/config/transform-mode.ts)
  requires configuration and user-tier consent.

## Failure scenario

If an invalid final value or destination were discovered after a prefix write,
the host could observe mixed output. Restoring a stale captured array after
supersession would also violate the contract.

## Timing windows and dependencies

The final check/write/promotion block and every rejection path matter. The
all-or-none claim rests on the define loop having no user-code path once its
precondition holds: own data slots on a plain extensible array with a writable
length and unpolluted built-in prototypes. The loop reads only own slots of the
plain candidate array; the candidate's member values are not read there.

## What a test must construct

Use invalid containers and candidates. After rejection, compare current
contents and array identity with an independent expected value. Exercise
clear, supersession, invalidation, and a between-pages source change before
application. Recipe integration must add the malformed-final-operation case;
the current candidate builder is exercised through `native_messages` and
`native_messages_delta` only.

## Investigation log

### Q: Does validation precede every possible partial publication?

- Sources examined: the replacement helper and its preconditions, the
  application block, the wrapper, and the witnesses below.
- Findings: Every precondition has a rejection witness that also asserts the
  host array's identity and contents. No test injects a throw inside the
  define loop; the argument for that case is the precondition proof, not a
  fault injection.
- Missing evidence: #538's malformed-final-operation candidate case.
- Conclusion: resolved as exercised for this revision's candidate shapes; the
  witness list is under the next question.

### Q: What bounds the candidate, and what is not checked on this path?

- Sources examined: `hostArrayReplacementRejection` and
  `replaceHostArrayContents` at the lines above, `buildNativeCandidate` at
  `rust-mode-transform.ts:578-626`, the application block at `:1434-1491`,
  and the witnesses below.
- Findings: `hostArrayReplacementRejection` takes no length and applies no
  candidate-length cap; the slot charge inside `buildNativeCandidate`
  (`:586`, `:601`, `:622`) is the only candidate bound. The define loop's only
  precondition is a `null` rejection. The candidate is not inspected before
  `assertNativeBoundary`, so a kept previous-output entry that gained an
  accessor is not refused on this path; `assertNativeBoundary` reads the head
  entries plainly. The between-pages witness shows a completed series refused
  at publication with identity kept. The numeric-accessor witness shows
  `hostArrayReplacementRejection` returning `prototype_accessor` for an
  accessor on `Array.prototype` or `Object.prototype` slot `0` while the
  define loop fills the destination without consulting it.
- Missing evidence: #538's malformed-final-operation candidate case.
- Conclusion (2026-09-13, revision-bound run at `d5a525e8`, after merging
  `origin/main` at `5def3c71`, 1122 pass, 0 fail): resolved as exercised for this revision's
  candidate shapes. Witnesses and markers:
  - `transform-capture.test.ts:783` "refuses a numeric accessor on a built-in
    prototype and defines slots without invoking it" (2 cases; marker:
    `hostRejection` is `prototype_accessor`, `target` equals `next` with
    `target[0]` the same object as `next[0]`, `counter.count` 0).
  - `:817` "reports a destination slot that stopped accepting writes and
    reads no candidate getter" (`element_not_writable`,
    `counter.count` 0); `:832` "replaces every slot and the length of an
    accepted destination in place" (`length` stays writable); `:843` "rejects
    inherited membership and does not consult its getter"; `:864` "accepts a
    plain extensible array and replaces its contents in place"; `:873`
    "rejects containers whose element or length assignment could throw"
    (`not_array`, `proxy`, `not_extensible`, `length_not_writable`,
    `element_not_writable`, `prototype`).
  - `rust-mode-transform.test.ts:2085` "publishes at the exact candidate
    charge and leaves the host array intact" and "declines one byte short of
    the candidate charge and leaves the host array intact". Marker:
    `started.promise` race, then a blocker lease reserves
    `remainingBytes - candidateLength * 8 + offset` (`:2116`); on decline the
    log names `native candidate array`, `output.messages[0]` is still
    `member`, and the NACK carries `candidate`.
  - `:2718` "declines a proxied or non-replaceable host container without
    dispatch" (`calls` 0, `failureCount` 0).
  - `:2843` "stops an in-flight pass when the session is cleared and keeps
    the host array intact" (`calls` 1 before `clearSession`).
  - `:2770` "rejects publication when a message is edited in place while the
    transform response is pending" (`calls` 1 before the edit).
  - `:2660` "declines before publication and NACKs known deliveries when the
    source changes between pages". Marker: the series completed with
    `transform_page_complete` true; `output.messages` is `messages`, length
    1, `[0]` is `member`, and the hook was never called.
  - `hook.test.ts:273` "preserves host array and returned payload identity
    through the actual <hook|wrapper>" (2 cases).
  - `messages-transform.test.ts:293` "keeps the current host contents when the
    inner hook mutates then throws"; `:321` "does not roll back a concurrent
    <replace-member|rebind-array> when the pending hook throws" (2 cases;
    `started.promise` race before the host edit); `:210` "checks only the
    return container after the hook publishes".

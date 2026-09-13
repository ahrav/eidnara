# publication-is-current-and-atomic

## Discovery trigger

Parent #525 requires complete validation before host writes, current ownership
at publication, and preservation of OpenCode's array identity on every outcome.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner`, parent `b0023512`.

- [transform-capture.ts](../../../../../packages/opencode-plugin/src/hooks/context/transform-capture.ts):
  `hostArrayReplacementRejection` (`:400-425`) rejects proxies, non-arrays,
  malformed or over-budget lengths, non-extensible arrays, a non-writable
  `length`, and any slot that is not writable and configurable, walking the
  destination's own descriptors only. `replaceHostArrayContents` (`:432-437`)
  is a define loop over own slots plus one `length` define; its preconditions
  (`:427-431`) are that the rejection check returned `null` and the candidate
  passed `inspectReferenceableMessages`.
- [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts):
  `buildNativeCandidate` reserves candidate slots (`:1425-1434`), the
  candidate is inspected (`:1436-1441`) before `assertNativeBoundary`
  (`:1443-1445`), the continuation shift runs on the staged memo
  (`:1446-1465`), `recheckCapture("publish")` and the container check run
  (`:1467-1471`), and then replacement, `nativeOutput`, `state.ordinals`,
  `state.initialized`, `wireCaches.set`, and `deliveries.applied` are assigned
  with no await between them (`:1475-1482`). Successful publication transfers
  the candidate array and `captured.snapshots` (through the pending wire
  cache's `rawContentSnapshots`, `:289`) to the 64-session `wireCaches` owner
  (`:126`, `:787`) and the promoted memo to `states`; the lease releases in
  `.finally` after that block (`:1520-1521`). This count-bounded retention is
  the current boundary; the separate optional-output byte budget is TE25 in
  #538.
- [messages-transform.ts](../../../../../packages/opencode-plugin/src/plugin/messages-transform.ts)
  catches hook errors, logs them, and returns the current `output.messages`
  (`:65-83`); it never assigns `output.messages`.
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
preconditions hold: own data slots on a plain extensible array with a writable
length, and a candidate whose members are plain data.

## What a test must construct

Use invalid containers and candidates. After rejection, compare current
contents and array identity with an independent expected value. Exercise
clear, supersession, and invalidation before application. Recipe integration
must add the malformed-final-operation case; the current candidate builder is
exercised through `native_messages` and `native_messages_delta` only.

## Investigation log

### Q: Does validation precede every possible partial publication?

- Sources examined: the replacement helper and its preconditions, the
  application block, the wrapper, and the witnesses below.
- Findings: Every precondition has a rejection witness that also asserts the
  host array's identity and contents. No test injects a throw inside the
  define loop; the argument for that case is the precondition proof, not a
  fault injection.
- Missing evidence: #538's malformed-final-operation candidate case.
- Conclusion (2026-09-13, revision-bound run, 1098 pass, 0 fail): resolved
  as exercised for this revision's candidate shapes. Witnesses and markers:
  - `transform-capture.test.ts:569` "bounds the candidate length at the slot
    budget and rejects malformed lengths"; `:606` "reports a destination slot
    that stopped accepting writes and reads no candidate getter"
    (`counter.count` 0); `:621` "replaces every slot and the length of an
    accepted destination in place"; `:632` "rejects inherited membership and
    does not consult its getter"; `:653` "accepts a plain extensible array and
    replaces its contents in place"; `:662` "rejects containers whose element
    or length assignment could throw" (each rejection reason named); `:581`
    `it.each` "bypasses inherited numeric setters for capture and
    publication" (inherited setter on `"0"`, `counter.count` 0).
  - `rust-mode-transform.test.ts:1916` "publishes at the exact candidate
    charge and leaves the host array intact" and "declines one byte short of
    the candidate charge and leaves the host array intact". Marker:
    `started.promise` race with `calls` 1, then a blocker lease reserves
    `remainingBytes - candidateLength * 8 + offset`; on decline the log names
    `native candidate array`, `output.messages[0]` is still `member`, and
    the NACK carries `candidate`.
  - `:2600` "declines a proxied or non-replaceable host container without
    dispatch" (`calls` 0, `failureCount` 0).
  - `:2725` "stops an in-flight pass when the session is cleared and keeps
    the host array intact" (`calls` 1 before `clearSession`).
  - `:1980` "refuses a kept previous-output entry that gained an accessor and
    invokes no hook" (`second.messages` equals its pre-pass members).
  - `hook.test.ts:273` "preserves host array and returned payload identity
    through the actual <hook|wrapper>" (2 cases).
  - `messages-transform.test.ts:254` "keeps the current host contents when the
    inner hook mutates then throws"; `:282` "does not roll back a concurrent
    <replace-member|rebind-array> when the pending hook throws" (2 cases;
    `started.promise` race before the host edit); `:171` "checks only the
    return container after the hook publishes".

# inbound-baseline-independent-of-output-base

## Discovery trigger

Parent #525 requires accepted submitted input to remain the inbound delta
baseline even when approved output differs from it.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner` after merging
`origin/main` at `5def3c71`. Line numbers were verified against that tree.

- In [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts),
  `buildWireCache` (`:263-288`) derives `rawContentSnapshots`, the CK and
  native fingerprints, and the combined `fingerprint` from the encoded
  submitted input and `captured.snapshots`. The pass builds its pending cache
  from the capture (`:1221-1230`, and `:1397-1401` on the full retry).
  `nativeOutput` is attached separately after host replacement (`:1477`).
  `computeWireDelta` (`:219`, called at `:997`) compares the next capture's
  snapshots with the previous cache's raw snapshots, not with `nativeOutput`.
  This is source evidence for the separation, not an end-to-end equality
  oracle.
- `state.forceFullWire = true` is set before the first daemon send (`:1361`)
  and cleared only on publication (`:1481`). A dispatched pass that declines
  afterwards leaves the daemon's committed snapshot ahead of the client's
  wire cache, and the next pass resends the full history instead of a delta
  against a baseline the client did not commit.
- Reachability is `explicit-config-only`: the Rust-mode
  [hook](../../../../../packages/opencode-plugin/src/hooks/context/hook.ts)
  reaches wire-cache construction through `run`;
  [resolveTransformMode](../../../../../packages/opencode-plugin/src/config/transform-mode.ts)
  requires configuration and user-tier consent.

## Failure scenario

If applied output replaced accepted raw input as the next inbound basis, an
append would produce a different approved prompt through delta and
full-request paths. The test must compare those paths rather than count calls.

## Timing windows and dependencies

Use an output-changing transform, append to raw input, and repeat with
optional-output-only eviction. Removing optional output must not replace or
erase the independent inbound basis merely to make a delta test pass. A
dispatched pass that declines after the daemon committed its snapshot must
not leave a delta baseline the client did not accept.

## What a test must construct

Keep an independent raw input fixture and approved-output expectation. After
the first output-changing transform, append to raw input. Compare the next
delta result with a full-request control under equivalent daemon policy state.
Check approved values and inbound fingerprints and frontiers, including unknown
fields. Repeat after optional-output-only eviction. The integration handoff
remains #538.

## Investigation log

### Q: Does the full-request control match the delta after an output-changing transformation?

- Sources examined: Parent #525's TE30 acceptance requirement, wire-cache
  construction and the delta computation, and the witnesses below.
- Findings: Cache fields have separate input and output sources. The
  executed tests publish output that differs from input and then dispatch
  later passes as deltas, but none compares a delta result with a
  full-request control, and none evicts optional output alone.
- Missing evidence: The #538 comparison and optional-output-only eviction
  case, with audited independent expectations.
- Conclusion: TE30 remains partial; the witness list is under the next
  question.

### Q: Can a declined dispatched pass leave a stale baseline?

- Sources examined: `rust-mode-transform.ts:263-288`, `:997`, `:1221-1230`,
  `:1361`, `:1397-1401`, `:1477`, `:1481`; the witnesses below.
- Findings: The cache derives from the capture and the encoded input, not
  from `nativeOutput`. The forced full send precedes the first dispatch, so a
  pass declined at publication cannot leave the client sending a delta
  against a snapshot only the daemon committed. That is a
  baseline-realignment control, not the delta-versus-full equality oracle
  TE30 requires.
- Missing evidence: The #538 comparison and optional-output-only eviction
  case, with audited independent expectations.
- Conclusion (2026-09-13, revision-bound run after merging `origin/main` at
  `5def3c71`, 1107 pass, 0 fail): TE30 remains partial. Related executed
  witnesses and what they do show:
  - `rust-mode-transform.test.ts:2901` "releases capture admission before
    the ACK so a paused ACK does not block the next pass". Marker: the first
    pass publishes `applied`, then a second pass over two rows dispatches;
    no delta/full comparison.
  - `:1344` "keeps applied note output when a newer pass starts during the
    ack". Marker: after two output-changing passes, the third body carries
    `tail_delta.native_replace_from` 1; still not an equality control.
  - `:1622` "in-place mutation of an older message forces a full send instead
    of a delta". Marker: an input-side edit yields a body without
    `tail_delta`; this guards the input baseline, not the output base.
  - `:803` "forces a full send after a dispatched delta pass is
    source-declined". Marker: `bodies[2].tail_delta` defined when the fake
    replaces `live[0]`; after the decline `forceFullWire` true; the next
    pass's `bodies[3]` has no `tail_delta` and its `native_messages` equal
    the new input (`:830-831`).
  - `:2470` (4 cases). Marker: `forceFullWire` equals `fault !== "clear"`
    after a `need_full_sync` retry is refused (`:2531`).
  Two transform calls do not establish equality; the comparison belongs to
  #538.

# inbound-baseline-independent-of-output-base

## Discovery trigger

Parent #525 requires accepted submitted input to remain the inbound delta
baseline even when approved output differs from it.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner`, parent `b0023512`.

- In [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts),
  `buildWireCache` (`:267-292`) derives `rawContentSnapshots`, the CK and
  native fingerprints, and the combined `fingerprint` from the encoded
  submitted input and `captured.snapshots`. The pass builds its pending cache
  from the capture (`:1219-1227`, and `:1392-1396` on the full retry).
  `nativeOutput` is attached separately after host replacement (`:1476`).
  `computeWireDelta` (`:994`) compares the next capture's snapshots with the
  previous cache's raw snapshots, not with `nativeOutput`. This is source
  evidence for the separation, not an end-to-end equality oracle.
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
erase the independent inbound basis merely to make a delta test pass.

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
- Conclusion (2026-09-13, revision-bound run, 1098 pass, 0 fail): TE30
  remains partial. Related executed witnesses and what they do show:
  - `rust-mode-transform.test.ts:2907` "releases capture admission before
    the ACK so a paused ACK does not block the next pass". Marker: the first
    pass publishes `applied`, then a second pass over two rows dispatches;
    no delta/full comparison.
  - `:1312` "keeps applied note output when a newer pass starts during the
    ack". Marker: after two output-changing passes, the third body carries
    `tail_delta.native_replace_from` 1, so the delta was computed against
    submitted input rather than the applied output's length; still not an
    equality control.
  - `:1590` "in-place mutation of an older message forces a full send instead
    of a delta". Marker: an input-side edit yields a body without
    `tail_delta`; this guards the input baseline, not the output base.
  Two transform calls do not establish equality; the comparison belongs to
  #538.

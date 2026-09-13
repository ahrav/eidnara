# previous-base-is-applied-and-live

## Discovery trigger

Parent #525 requires retained, validated, successfully applied output before
advertising `previous_output_revision`, and validation again before its keeps.
An applied object can still be mutated later.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner`, parent `b0023512`.

- In [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts),
  `buildNativeCandidate` (`:1425-1434`) receives the previous cache's
  `nativeOutput` and `fingerprint`, checks `delta.after` and the prefix range,
  reserves candidate slots, and copies references from the previous output.
  The candidate inspection at `:1436-1441` refuses kept entries that gained an
  accessor, proxy, or hook, so a hooked kept entry cannot be published or read.
- `pendingWireCache.nativeOutput = candidate` runs after host replacement
  (`:1476`); a delta pass that failed leaves the previous cache's
  `nativeOutput` in place only when the pass never replaced it.
- The cache fingerprint describes inbound wire state (`:290`). Matching it is
  not a content validation of previously applied output or a recipe revision.
- No code compares a kept entry's current data values with the values approved
  when it was published. That validation is the #538 obligation.
- Reachability is `explicit-config-only`: the Rust-mode
  [hook](../../../../../packages/opencode-plugin/src/hooks/context/hook.ts)
  reaches the candidate builder;
  [resolveTransformMode](../../../../../packages/opencode-plugin/src/config/transform-mode.ts)
  requires configuration and user-tier consent.

## Failure scenario

After a successful publication, the host mutates a retained payload's data
values. Reusing it because the cache fingerprint still matches would apply
values that no longer equal the approved base. A rejected output must never
become eligible either.

## Timing windows and dependencies

Validate before constructing the next request and again before using previous
keeps. Include mutations during the intervening await and invalidation or
eviction. Mutation removes eligibility; it must not repair host-owned objects.

## What a test must construct

Publish output distinct from raw input and retain its independent expected
values. Mutate a kept entry's data values before the next request, then in a
separate case during the request. Check previous-base eligibility and
reconstruction against that expectation, not only reference identity or hook
refusal. Include a valid unmodified base and an unapplied response as controls.

## Investigation log

### Q: Does applied-prefix identity establish TE21?

- Sources examined: Parent #525's capture/publication contract, the candidate
  builder and cache assignment, and the witnesses below.
- Findings: Identity and hook refusal are exercised. Value validation of kept
  entries is not implemented, so no test can exercise it.
- Missing evidence: #538's before-advertisement and before-reuse validation
  and its value-mutation witnesses.
- Conclusion (2026-09-13, revision-bound run, 1098 pass, 0 fail): TE21 remains
  partial. Executed witnesses and markers:
  - `rust-mode-transform.test.ts:3106` "preserves the payload identity of
    kept and returned messages on publication". Marker:
    `secondOutput.messages[0]` is `returned[0]`, `[1]` is `returned[1]`,
    `entries.size` 2.
  - `:1980` "refuses a kept previous-output entry that gained an accessor and
    invokes no hook". Marker: `expect(hook).not.toHaveBeenCalled()`,
    `second.messages` equals its pre-pass members, NACK of `kept-attempt`,
    `failureCount` 1 (a daemon response was rejected, so this counts as a
    failure rather than a decline).
  - `:1210` "applies a native_messages_delta in place and acks its note
    deliveries". Marker: `output.messages[0]` is `first[0]`.
  - `:1574` "rejects a delta whose prefix fingerprint is not acknowledged".
    Marker: the throw names the acknowledged output and
    `lease.chargedBytes` is 0 afterwards.
  No executed test mutates a kept entry's data values before reuse; that case
  is assigned to #538.

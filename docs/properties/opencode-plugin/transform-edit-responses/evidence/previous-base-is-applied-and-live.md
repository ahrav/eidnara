# previous-base-is-applied-and-live

## Discovery trigger

Parent #525 requires retained, validated, successfully applied output before
advertising `previous_output_revision`, and validation again before its keeps.
An applied object can still be mutated later.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner` after merging
`origin/main` at `5def3c71`. Line numbers were verified against `d5a525e8`.

- In [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts),
  `buildNativeCandidate` (`:578-626`) receives the previous cache's
  `nativeOutput` and `fingerprint` (call at `:1436-1445`), checks
  `delta.after` against `previous.fingerprint` and `replace_from` against the
  prefix range (`:609-621`), reserves candidate slots (`:622`), and copies
  references from the previous output by slice (`:625`).
- The candidate is not inspected before `assertNativeBoundary`
  (`:1446-1449`), which reads the candidate's head entries plainly
  (`:377-383`). A kept previous-output entry that gained an accessor is
  therefore not refused on this path. Previous-output validation, including
  kept prefix entries, is #538's TE21 work.
- `pendingWireCache.nativeOutput = candidate` runs after host replacement
  (`:1480`); a delta pass that failed leaves the previous cache's
  `nativeOutput` in place only when the pass never replaced it. A dispatched
  pass that declines sets nothing here, but `state.forceFullWire` is already
  true (`:1363`), so the next pass sends the full history and does not
  advertise the previous base.
- The cache fingerprint describes inbound wire state (`:286`). Matching it is
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
values or installs an accessor on it. Reusing it because the cache fingerprint
still matches would apply values that no longer equal the approved base, or
run the accessor inside `assertNativeBoundary` or later consumers. A rejected
output must never become eligible either.

## Timing windows and dependencies

Validate before constructing the next request and again before using previous
keeps. Include mutations during the intervening await and invalidation or
eviction. Mutation removes eligibility; it must not repair host-owned objects.

## What a test must construct

Publish output distinct from raw input and retain its independent expected
values. Mutate a kept entry's data values before the next request, then in a
separate case during the request, and in a third case install an accessor on
it. Check previous-base eligibility and reconstruction against that
expectation, not only reference identity or fingerprint match. Include a valid
unmodified base and an unapplied response as controls.

## Investigation log

### Q: Does applied-prefix identity establish TE21?

- Sources examined: Parent #525's capture/publication contract, the candidate
  builder and cache assignment, and the witnesses below.
- Findings: Identity preservation and the fingerprint and range checks are
  exercised. Value validation of kept entries is not implemented, so no test
  can exercise it, and nothing on this path refuses a hooked kept entry.
- Missing evidence: #538's before-advertisement and before-reuse validation
  and its value-mutation witnesses.
- Conclusion: TE21 remains partial; the witness list is under the next
  question.

### Q: What does the owner check about the previous base?

- Sources examined: `buildNativeCandidate` at `rust-mode-transform.ts:578-626`,
  the application block at `:1434-1449`, `assertNativeBoundary` at
  `:377-383`, `:1363`, `:1480`; the witnesses below.
- Findings: The fingerprint and range checks and the identity-preserving
  slice are the only checks on the previous base. Nothing on this path reads
  a kept entry's values or refuses a hooked kept entry before
  `assertNativeBoundary` reads the head entries plainly.
- Missing evidence: #538's before-advertisement and before-reuse validation,
  its value-mutation witnesses, and a refusal for a hooked kept entry.
- Conclusion (2026-09-13, revision-bound run at `d5a525e8`, after merging
  `origin/main` at `5def3c71`, 1122 pass, 0 fail): TE21 remains partial. Kept-prefix
  validation is #538's TE21. Executed witnesses and markers:
  - `rust-mode-transform.test.ts:3224` "preserves the payload identity of
    kept and returned messages on publication". Marker:
    `secondOutput.messages[0]` is `returned[0]`, `entries.size` 2.
  - `:1242` "applies a native_messages_delta in place and acks its note
    deliveries". Marker: `output.messages[0]` is `first[0]`.
  - `:1606` "rejects a delta whose prefix fingerprint is not acknowledged".
    Marker: the throw names the acknowledged output and
    `lease.chargedBytes` is 0 afterwards.
  - `:1562` "retries with full arrays when a delta response omits native
    content". Marker: `bodies[1].tail_delta` present, `bodies[2].tail_delta`
    undefined, `consecutiveFailures` 0.
  No executed test mutates a kept entry's data values or installs a hook on
  one before reuse; those cases are assigned to #538.

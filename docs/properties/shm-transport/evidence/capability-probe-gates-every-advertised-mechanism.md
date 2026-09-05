# capability-probe-gates-every-advertised-mechanism

## Discovery trigger

`docs/shm-transport.md:33-40` (source tree; not at HEAD) enumerates eight steps that
`probeCapabilities()` must pass, and line 42 states "Any failure returns
`available: false` with a bounded reason." That is a universally quantified
claim over the eight steps, so each step was matched to the code that gates it.

## Evidence trail

`packages/shm-native/index.ts:319-443` is the whole probe. Its exits, in
source order, with the line carrying the `available:` field:

| Exit | Line | Reason | Documented step |
| --- | --- | --- | --- |
| platform is not Linux | 118 | `platform_unsupported` | prose at doc line 31, not a numbered step |
| not Bun and `process.release.name === "node"` | 122 | `node_detachment_unavailable` | **not enumerated anywhere in the doc** |
| addon did not load | 129 | `addon_unavailable` | 1 |
| `napiVersion < 8` | 134 | `napi_8_unavailable` | 2 |
| `!exactBounds` | 149 | `external_exact_bounds_unavailable` | 3 and 4 |
| `!transferPrevention` | 169 | `transfer_prevention_unavailable` | 6 |
| `!aliasesDetached` | 193 | `detachment_unavailable` | 7 |
| any throw | 215 | `runtime_mechanism_unavailable` | catch-all, covers 5 |
| success | 203 | — | 8 is **reported, not gated** |

The success return at lines 202-210 sets `available: true` at line 203 and then
records the eighth mechanism as a field inside the same object literal:

```ts
cleanupHooks: typeof native.registerCleanupProbe === "function",
```

Line 209. Nothing reads that expression before `available: true` is committed. A
runtime whose addon does not export `registerCleanupProbe` is advertised as
capable with `cleanupHooks: false`.

Two refinements to how the first seven steps are covered, established by reading
the conjunctions rather than the return count:

- Step 3, "Create a 31-byte external `Uint8Array`", has no dedicated gate. It is
  measured into `externalArrayBuffer` at lines 141-142 and folded into step 4's
  predicate at lines 143-146 (`exactBounds = externalArrayBuffer && ...`), so a
  step 3 failure exits at line 149 under step 4's reason. It is gated, but not
  with its own bounded reason.
- Step 5, "Create `subarray`, `DataView`, and `Buffer` aliases" (lines 157-159),
  has no dedicated gate either. A throw there reaches the outer `catch` at line
  211 and reports `runtime_mechanism_unavailable`. A non-throwing but broken
  alias is caught indirectly at step 7, because `aliasesDetached` (lines
  178-190) asserts `subarray.byteLength === 0` and `bufferAlias.byteLength === 0`
  after detachment.

So the outcome the catalog states holds — steps 1 through 7 cannot yield
`available: true` — but the shape is five dedicated gates plus a catch-all, not
one gate per step.

Existing coverage is closer than the catalog credits, and still cannot fail.
`packages/shm-native/tests/mechanism.ts:28-44` asserts exactly the required
implication: inside `if (result.available)` it asserts
`expect(result.cleanupHooks).toBe(true)` at line 31. That assertion is correct
and would catch the defect — on a runtime that lacks the hook. On every runtime
in CI the addon exports `registerCleanupProbe` (`src/lib.rs:535`), so
`cleanupHooks` is always `true` and the assertion passes without discriminating.
`packages/shm-native/tests/capability.ts:9-59` asserts channel counts around
the probe and that `createTestPair` throws `/capability unavailable/` when the
probe reports unavailable; it does not test gating.

## Failure scenario

A runtime ships an addon build without `registerCleanupProbe`, or the export is
renamed. `probeCapabilities()` reaches line 202, returns `available: true`, and
sets `cleanupHooks: false`. A caller that trusts `available` proceeds to attach a
channel. N-API asynchronous cleanup never runs at environment teardown, so the
mapping and any attached external references are never revoked on the
environment thread — the exact condition the documented close ordering exists to
prevent. `mechanism.ts:39` would catch it, but only if the test runs on that
runtime.

## Timing windows and dependencies

None internal to the probe; it is a straight-line sequence with no concurrency.
The fault required is environmental: a runtime or build lacking one enumerated
mechanism. That is also the dependency that makes the existing assertion
non-vacuous, so the property and its check share a single enabling condition.

## What a test must construct

1. A probe invocation against a native surface with `registerCleanupProbe`
   absent, asserting `available === false` and a non-empty `reason`. A stub
   addon object satisfying the rest of the interface is sufficient; a real
   runtime is not required, but the probe currently obtains its addon through
   the module-local `addon()` helper (line 127), so a seam is needed.
2. Per enumerated step, a stub that fails only that step, asserting
   `available === false` and a `reason` distinct from the other seven. This is
   what would expose the shared reason on step 3 and the catch-all on step 5.
3. A control asserting the two undocumented pre-gates are intentional: a
   non-Bun Node runtime on Linux currently returns
   `node_detachment_unavailable` before step 1, which no enumerated step
   describes.

## Investigation log

### Q: Is the documented eight-step list normative on ordering as well as on membership? The code's order differs from the doc's numbering.

- Sources examined: `docs/shm-transport.md:29-42` (source tree; not at HEAD);
  `packages/shm-native/index.ts:319-443` line by line;
  `packages/shm-native/src/lib.rs:534-537` for the `registerCleanupProbe`
  export; `packages/shm-native/tests/mechanism.ts:28-44`;
  `packages/shm-native/tests/capability.ts:1-67`.
- Findings: on membership, the code adds one gate the list does not contain
  (`node_detachment_unavailable`, lines 120-126) and omits a gate for step 8. On
  ordering, I could not reproduce the divergence the catalog asserts. Steps 1
  through 8 appear in the code in the documented sequence: addon load (127-129),
  N-API version (131-139), external probe creation (140), exact bounds
  (143-155), alias creation (157-159), untransferability and `structuredClone`
  (160-176), detachment and alias invalidation (177-201), cleanup hooks (209).
  The platform check runs before addon loading, which doc line 42 explicitly
  sanctions. The only sequencing divergence is the undocumented runtime-family
  gate inserted ahead of step 1.
- Missing evidence: nothing states whether the list is meant to be exhaustive of
  gates or only of mechanisms. An extra gate is conservative with respect to the
  guarantee, so it is a documentation gap rather than a safety defect; that
  reading is an inference, not a recorded decision.
- Conclusion: unresolved, needs the doc owner to state whether the list is
  exhaustive. One catalog correction is settled independently: the claim that
  "the code's order differs from the doc's numbering" is not supported at
  `9c1eb4d1` for steps 1-8. The substantive finding — step 8 is reported at line
  209 and never gated, while line 203 has already committed `available: true` —
  is confirmed exactly as stated.

### Q: Is the cleanup hook gated or only reported at HEAD? (added 2026-09-05)

- Checked: `probeCapabilities` (`packages/shm-native/index.ts:319`) returns `available: false` with `reason: "cleanup_hooks_unavailable"` when `typeof native.registerCleanupProbe !== "function"` (`:415-426`) and `available: true, cleanupHooks: true` only otherwise (`:427-435`). `capableAddon` (`:276-281`) calls the probe on every channel construction and throws `capability_unavailable` when it fails, so the probe is on the shipped path.
- Conclusion: gated. The record is refreshed; Reachability moves to default-production.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 12, `packages/shm-native/index.ts:108-230` now `packages/shm-native/index.ts:319-443`: At HEAD the cleanup hook is gated, not only reported: `probeCapabilities` returns `available: false` with `cleanup_hooks_unavailable` (`:415-426`) before the success return (`:427-435`), and it probes the hook by calling `native.probeCleanupHooks()` (`:408-414`).
  - line 64, `packages/shm-native/tests/capability.ts:8-33` now `packages/shm-native/tests/capability.ts:9-59`: At HEAD the unavailable branch asserts `createTestPair` throws `/shared-memory native addon|shared-memory native startup failed/` (`:55-58`), not `/capability unavailable/`.
  - line 133, `:329-340` now `:403-414`: At HEAD the gate calls `native.probeCleanupHooks()` inside a `try` (`:396-402`) and refuses when it throws; it does not test `typeof native.registerCleanupProbe`.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 5, `docs/shm-transport.md:33-40` (eight enumerated probe steps): `docs/shm-transport.md` no longer enumerates the probe's steps; at HEAD it lists six setup phases (`:38-47`) and no capability-probe list.
  - line 105, `docs/shm-transport.md:29-42` (documented probe step list): `docs/shm-transport.md` no longer enumerates the probe's steps or the any-failure sentence; the setup phases are at `:38-47`.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.

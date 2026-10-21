# memory-seed-fixture-writes-repo-source-kind-under-code-observed

## Discovery trigger

Lens: existing tests and wildcard. The brief named "e2e memorySeed and
extractM0 as seams". U6 step 3 proposes seeding "one directive (via
`directive.record` through the test kernel client, or `memorySeed` for the
directive row)". Reading the fixture showed what a seeded row is, which
decides whether `memorySeed` can stand in for a directive under KTD5's
`source_kind` partition.

## Evidence trail

- TypeScript seam: `packages/e2e-tests/src/rust-runner/hermetic-host.ts:768-777`
  `memorySeed(anchor, objectId, category, text)` sends control command
  `memory-seed` (`:162`) and parses a `MemoryAdmission`
  (`effectiveMaturity`, `disposition`, `visibility`).
- Fixture: `crates/daemon/examples/direct_host_fixture.rs:755-783`
  `memory_seed`: resolves the anchor's `scope_id`, `ensure_domain(MEMORY_DOMAIN)`,
  `insert_decision(memory_decision_spec(object_id, &scope, category, text))`,
  then `observe_code(envelope, object_id, "<object_id>-observed",
  (SourceClass::TrustedLocalCode, TaintClass::CurrentCode))`.
- `memory_decision_spec` -> `decision_spec_in_domain`
  (`crates/daemon/src/lib.rs:13785-13816`): `decision_kind: kind`,
  `source_kind: "repo"`, `source_id: "<object_id>-lineage"`, `payload.summary
  = text`, `rationale` empty, `scope_id: Some(scope)`.
- `observe_code` (`lib.rs:13853-13889`): inserts an observation with
  `observation_kind: "code_present"`, `source_kind: "repo"`, `source_id:
  "<object_id>-lineage"`, `source_revision: 1`, then `record_admission` with
  `EventKind::CodeObserved` and the given classes (the exact-lineage trigger
  S1 owns).
- Frame cap: `direct_host_fixture.rs:66` `MAX_CONTROL_LINE = 64 * 1024`;
  `:854`, `:867` refuse oversized frames. The delivery catalog's
  `cf-memory-credit-requires-admitted-visible-content` records the
  consequence (a 64 KiB-plus-qualifier row cannot be seeded).
- Observed results: `compression-fidelity-memory.test.ts:200-240`
  `expect([admitted.effectiveMaturity, admitted.visibility]).toEqual(["verified",
  "automatic"])`; the wrong-project seed uses an anchor in another project
  and is excluded by scope.
- `extractM0` (`packages/e2e-tests/src/cache-analysis.ts:284-309`) returns
  the whole text of the first message containing `<session-history>`;
  `compose_m0` appends `<project-memory>` to that same text
  (`m0_compose.rs:239-243`), so the block is inside the returned string
  and a block-scoped regex (`PROJECT_MEMORY` at
  `compression-fidelity-memory.test.ts:34`) is the narrower extraction.

## Failure scenario

A parity test seeds "a directive" through `memorySeed`, runs one pass per
harness, and compares rule object ids. Under KTD5 the required phase selects
`source_kind in {user_directive, repository_policy}`; the seeded row has
`source_kind = repo`, so it packs in the optional phase and renders under
its category heading, not `USER_DIRECTIVES`. The test would pass while
exercising neither the required phase nor the disclosure line; AE8 would be
vacuous for the directive channel.

## Timing windows and dependencies

None. The seed commits under a per-call token (`admit(kernel, token, ...)`)
so repeated seeds with the same `objectId` are the fixture's concern.

## What a test must construct

1. Seed one row; read it through the test kernel client on `explicit_search`
   with `object_ids: [objectId]`; assert `source_kind: "repo"`, `source_id:
   "<objectId>-lineage"`, `source_revision: 1`, and the admission's classes
   `trusted_local_code`, `current_code`.
2. Attempt a seed whose text pushes the control line to 64 KiB; assert the
   control call fails with the oversized error.
3. Coverage: `memory_seed_row_read_back` fires in case 1.

## Investigation log

### Q: Can `memorySeed` produce a `user_directive` row?

- Sources examined: `hermetic-host.ts:768-777`; `direct_host_fixture.rs:755-783`;
  `lib.rs:13785-13816`, `:13853-13889`.
- Findings: `source_kind` is the literal `"repo"` in both the decision and
  the observation; the event is `CodeObserved`; no parameter selects either.
- Missing evidence: none.
- Conclusion: resolved with answer: no. AE8 needs the directive route or a
  seam that takes `source_kind` and the admission event.

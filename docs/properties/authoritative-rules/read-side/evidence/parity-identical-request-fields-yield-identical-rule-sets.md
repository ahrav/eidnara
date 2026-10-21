# parity-identical-request-fields-yield-identical-rule-sets

## Discovery trigger

Plan requirement R13 ("For the same bound project, pass context, and
`is_subagent` value, OpenCode and Pi receive identical rule object ids and
identical disclosure lines") and KTD8 ("Parity is daemon-owned. Selection,
packing, and m0 composition stay in the pass"); acceptance example AE8; U6
test scenarios including the negative control. Serves R13; the source
brief's "Both harnesses produce consistent eligibility and delivery
outcomes".

## Evidence trail

- `crates/daemon/src/canonical_memory.rs:141-212`: the read and the
  snapshot are harness-agnostic; the function takes the binding and budget
  only. The proposed context adds `cwd` and the window, both request
  fields.
- `crates/daemon/src/lib.rs:9338-9345`: one pinned read per pass feeds
  every memory surface.
- `docs/host-wire-protocol.md:978` ("Native derivation"): for `pi` and
  `opencode-aisdk` the daemon derives the CK window from `native_messages`
  through the profile's codec; "Every later rule of this section reads the
  derived window as `messages`". Tool-call paths are read from the derived
  window, so parity is over the derived window.
- `docs/properties/daemon/decisions/catalog.md:1242`
  `codec-b-opencode-hides-four-part-types-from-every-transform-decision`
  and `:1185` `codec-b-pi-decoder-drops-unrecognised-entry-types-without-a-
  record`: the two codecs drop different native shapes, so "the same
  window" at the host may derive to different CK windows.
- `packages/pi-plugin/src/transform/pi-pass-inputs.ts:110` and
  `packages/opencode-plugin/src/hooks/context/transform-pass-fields.ts:
  55-75`: the two field builders; the `cwd` field must be added to both.
- `packages/e2e-tests/src/cache-analysis.ts:307` (plan-cited `extractM0`)
  and the two harness fixtures (plan-cited `compression-fidelity-pi.test.ts:
  155-165`, `compression-fidelity-memory.test.ts:70-73`): the oracle
  extraction and the fixtures a parity test composes.
- Plan Scope Boundaries: "Plugin-side filtering or ranking of rows:
  duplicates the selection oracle in two languages" is considered and not
  built, which is the design fact this record protects.

## Failure scenario

Legal witness: one directive D and one repository rule R (`dir:crates/
storage`). Pi pass and OpenCode pass with `cwd = <root>/crates/storage`,
the same tool call naming `crates/storage/src/lib.rs`, `is_subagent:
false`, same snapshot. Both m0 blocks contain `{D, R}`; neither has a
disclosure line; `rules_served == 2` on both.

Negative control: the OpenCode pass omits `cwd` and its window has no tool
call; its block contains `{D}` only and `context_resolved.cwd` is null.
The difference is attributable.

Smallest violating traces:
1. Plugin-side filter: the Pi plugin strips rows whose text matches its own
   `AGENTS.md` injection to avoid duplication (a deferred idea in the plan);
   OpenCode does not. Same request, different blocks. Parity fails by
   design drift, not by daemon defect.
2. Harness-conditional selection: the daemon branches on
   `serializer_profile` anywhere in the rule path (for example a different
   token estimator per profile). The served prefix differs under budget
   pressure.
3. Codec asymmetry: a tool call carries `file_path` inside a part type one
   codec drops; the derived windows differ; the ancestor sets differ; R is
   served on one harness only. The request fields were "the same" at the
   host, so a naive parity statement is false; the record states parity
   over the derived CK window and `context_resolved.paths` exposes the
   difference.
4. Snapshot skew: a commit lands between the two passes; one sees D' (a
   revision). The test must pin `known_as_of` or seed before both.
5. Order nondeterminism: see `packing-required-order-is-total-and-identical-
   across-passes`; equal sets but different served prefixes under pressure.

## Timing windows and dependencies

- Two passes on two sessions are not serialized against each other (the
  lane is per session, 7.10.1); a concurrent N1 or N2 write between them
  changes the snapshot legitimately.
- Depends on `parity-is-subagent-true-suppresses-m0-and-pi-sends-false`
  (parity is asserted only for equal `is_subagent`) and on
  `parity-pi-context-handler-registers-whenever-project-binds` (Pi must run
  a pass at all under the tested configuration).

## What a test must construct

1. E2E `rule-parity.test.ts`: seed one project with a committed `AGENTS.md`
   under `crates/storage/` and one directive (through `directive.record` on
   the test kernel client, or `memorySeed`); run one pass through the Rust
   harness with `CaseDriver` and one through `PiTestHarness` with the same
   `cwd` and a window containing one tool call naming
   `crates/storage/src/lib.rs`; `extractM0` both; assert equal object-id
   sets, byte-equal disclosure lines (or both absent), equal
   `project_memory.rules_served` and `rules_omitted`, and equal
   `context_resolved.paths`.
2. Negative control: repeat with one harness omitting `cwd` and no tool
   call; assert the sets differ and `context_resolved.cwd` differs; this
   proves the oracle can see a difference.
3. Pressure variant: seed enough rules to exceed the reserve; assert equal
   served prefixes and equal `rules_omitted`.
4. Static check: no code under `packages/*/src` filters or sorts
   `<project-memory>` rows after receipt (grep for the wrapper literal in
   consumers).
5. Pin `known_as_of` equality across the two passes or seed before both and
   write nothing between.

## Investigation log

### Q: Is parity defined over host windows or derived CK windows?
- Sources examined: 7.10.3 Native derivation; the two codec records in
  `docs/properties/daemon/decisions/catalog.md`.
- Findings: the daemon reads tool-call paths from the derived window; the
  codecs differ in what they drop.
- Conclusion: resolvable at implementation - state R13's "pass context" as
  the derived window plus `cwd`; the e2e must use a tool-call shape both
  codecs preserve, and a second case can document a known codec
  asymmetry if one is found.

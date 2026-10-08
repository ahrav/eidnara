# cf-shipped-recovery-capabilities

Repository: `/local/home/ahrav/scratch/eidnara`.
Inspected revision: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
Date: 2026-09-19. Exercise: not yet. Existing checks are unaudited.

## Discovery trigger

R4/R10 and KTD4 forbid claiming agent expansion or Pi history transformation
from internal functionality. The
[settled contract](https://github.com/ahrav/eidnara/issues/707), "Implementation
Decisions" KTD4 and "Provenance and reusable catalog"
(historical plan lines 114-116, 156, 225-226),
requires capability-bound reporting and invalidation when shipped capability
changes. Protocol, lifecycle, security, and compatibility passes supply this
record. No permanent prohibition on adding capabilities is inferred.

## Evidence trail

- [OpenCode registry, lines 34-59](../../../../../packages/opencode-plugin/src/plugin/tool-registry.ts#L34)
  returns no tools when disabled. When enabled, it assembles reduce, note,
  search, and memory; compaction-off removes reduce only. There is no
  dedicated native source-expansion tool in that registry.
- [Registry tests, lines 32-37, 69-119, 127-164](../../../../../packages/opencode-plugin/src/plugin/tool-registry.test.ts#L32)
  enumerate those IDs and schemas. The old `ctx_*` wording in test names
  is not the API: the expected IDs are `eidnara_*`. Complete enumeration
  catches an added or renamed expansion tool better than one absent name.
- [Search wrapper, lines 13-41](../../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.ts#L13)
  exposes memory-only sources and calls `executeEidnaraSearch`. Its
  [execution, lines 172-238](../../../../../packages/opencode-plugin/src/tools/eidnara-search/execute.ts#L157)
  performs kernel-client reads. The
  [client, lines 520-537](../../../../../packages/opencode-plugin/src/shared/kernel-client/client.ts#L520)
  sends `kernel.read`, not an `eidnara_search` facade request.
- [Separate daemon facade, lines 11887-11945](../../../../../crates/daemon/src/lib.rs#L11887)
  calls `search_history_segments_and_notes_for_session` and emits history
  title/body or note snippets. A direct facade test is not a registered
  model-tool witness and does not expose exact native occurrence bytes.
- [Memory ranking, lines 253-264](../../../../../packages/opencode-plugin/src/tools/eidnara-search/kernel-memory-search.ts#L253)
  emits exact object-id matches over memory decisions. The word `exact`
  here does not certify native occurrence/revision fidelity.
- [Pi entrypoint, lines 84-89, 367-370](../../../../../packages/pi-plugin/src/index.ts#L84)
  declared `PI_TRANSFORM_AVAILABLE = false` at the inspected revision and
  forced compaction off even when requested. Since `4ba36413a` (#850) it
  declares `true`: the Pi `context` handler folds history through the
  daemon's window protocol, so the requested compaction setting decides
  whether Eidnara owns compaction. [Lines 457-469](../../../../../packages/pi-plugin/src/index.ts#L457)
  pass that state to tool registration; the
  [tool gate, lines 96-98](../../../../../packages/pi-plugin/src/tools/index.ts#L96)
  omits reduce when compaction is off.
- [Pi test, lines 59-78](../../../../../packages/pi-plugin/src/index.test.ts#L59)
  now requires the flag to be `true` and checks that an acknowledged
  eviction answers compaction. This is the changed Pi gate the record
  names: the pinned Pi-unsupported expectation is invalidated and needs
  review. No fidelity scenario replays Pi history, so Pi still earns no
  P1-P5 coverage from this catalog.
- [Default configuration, line 335](../../../../../packages/opencode-plugin/src/config/schema/eidnara.ts#L335)
  enables the plugin. The registry and startup paths are production evidence
  for the expected tool surface under the captured configuration.

Reachability is `test-only`: the assertion compares a replay report with
registry inputs and their expected tool/argument/source surface. It does not
claim that production automatically emits this capability report. Production
registration remains the evidence source, not the property being classified.

## Failure scenario

A fixture reads native evidence through `KernelStore` or invokes the daemon
facade directly, then reports agent-side exact recovery. Another fixture
feeds Pi-shaped source records through Rust and reports Pi tier coverage.
The internal operations may be valid, but neither proves the shipped consumer
can reach the corresponding capability.

The competing explanation is that an expansion path has shipped. Check the
whole registry, schemas, execution route, and Pi gate at the evaluated build.
An old unsupported baseline must fail when this evidence changes. Do not keep
passing by skipping the new path or regenerating the expected unsupported row.

## Timing windows and dependencies

Bind capability evidence to the same inspected revision and configuration as
the invocation under evaluation. Include plugin-disabled and compaction-off
states to avoid confusing missing registration with unavailable data.
No model call, production export, or wire change is needed to inspect these
boundaries. A future capability needs its own approved contract and witnesses.

## What a test must construct

1. Reuse complete registry/schema assertions and verify the registered
   callback reaches `executeEidnaraSearch` and `kernel.read`. Feed those
   captured inputs to the report check; compare the expected registry surface
   for enabled, disabled, and compaction-off configurations.
2. Reject a direct same-name facade call as consumer-tool evidence, even if
   its returned history snippet is relevant.
3. Keep a successful privileged C6 read and an unsupported consumer exact
   expansion as two independent observations of the same scenario.
4. Reuse the Pi entrypoint test. Report unsupported Eidnara transformation,
   not synthetic P1-P5 success or a claim that Pi native compaction is absent.
5. Make added tool/source capability or a changed Pi gate invalidate the
   pinned expectation for review. Do not promise or implement that capability.

## Investigation log

### Q: Does the same-name facade establish registered recovery?

- Sources examined: registry, search wrapper/executor, kernel client, daemon
  facade, and Pi entrypoint cited above.
- Findings: the OpenCode callback reaches `kernel.read` and ranks memory
  decisions. The daemon facade is a different history/note path. Pi's
  transform gate is false regardless of requested compaction.
- Missing evidence: none for those code-level capability distinctions at
  this revision. A captured replay report has not been executed or audited.
- Conclusion: resolved for the inspected build. Exact expansion and Pi
  Eidnara transformation remain explicitly unsupported in this portfolio;
  a capability change invalidates that expectation rather than the new code.

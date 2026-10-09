# cf-visible-memory-recovery

Repository: `/local/home/ahrav/scratch/eidnara`.
Inspected revision: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
Date: 2026-09-19. Exercise: not yet. Existing checks are unaudited.

## Discovery trigger

The [settled contract](https://github.com/ahrav/eidnara/issues/707), "Materiality
and minimum meaning by tier", "Implementation Decisions" KTD4, and "Milestone
boundaries and dependencies" U3 (historical plan lines 78-87, 156, 225),
requires discoverability from the actual invocation and allows explicit
memory search to recover a decision/rationale, not an exact transcript.
Architecture, resource, protocol, and product lenses identify the missing
join between a visible cue, the registered call, and its delivered result.

Share the source/admission/invocation fixture with delivery's
[cf-memory-credit-requires-admitted-visible-content](../../delivery/catalog.md#cf-memory-credit-requires-admitted-visible-content).
Delivery owns admission, automatic inclusion, and pressure setup. Recovery
adds visible-argument provenance and explicit search delivery, then consumes
[the canonical disposition predicate](cf-unavailable-evidence-no-credit.md).
Do not build a second fixture or a second three-way disposition oracle.

## Evidence trail

- [Configuration, line 335](../../../../../packages/opencode-plugin/src/config/schema/eidnara.ts#L335)
  defaults the plugin to enabled. The
  [registry, lines 34-59](../../../../../packages/opencode-plugin/src/plugin/tool-registry.ts#L34)
  registers search with a kernel client and session/project resolvers when
  enabled; search is not removed by the compaction-off branch.
- [Search schema/wrapper, lines 13-41](../../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.ts#L13)
  exposes `query`, `limit`, and memory-only `sources`, then invokes
  `executeEidnaraSearch` and returns its text.
- [Execution, lines 142-181](../../../../../packages/opencode-plugin/src/tools/eidnara-search/execute.ts#L142)
  resolves the session/project and creates the kernel client. At
  [lines 200-238](../../../../../packages/opencode-plugin/src/tools/eidnara-search/execute.ts#L200)
  object IDs use chunked filtered reads, text queries use `explicit_search`,
  and nonavailable states remain explicit unless the no-consumer fallback
  succeeds with its freshness note.
- [KernelClient, lines 520-562](../../../../../packages/opencode-plugin/src/shared/kernel-client/client.ts#L520)
  serializes these reads as `kernel.read`, preserving surface and optional
  object IDs. This completes the registered-tool transport trace.
- [Daemon read, lines 390-478](../../../../../crates/daemon/src/kernel_routes/read.rs#L390)
  binds the request, applies the requested freshness gate, reads project-visible
  rows, and bounds serialized output. This is not native artifact expansion.
- [Memory ranking, lines 172-174, 248-285](../../../../../packages/opencode-plugin/src/tools/eidnara-search/kernel-memory-search.ts#L172)
  uses memory decision summaries/rationales. Exact object-id matching is a
  lookup mode. [Lines 188-229](../../../../../packages/opencode-plugin/src/tools/eidnara-search/kernel-memory-search.ts#L178)
  render rejected approaches distinctly and carry their rationale.
- [Delivery, lines 151-169, 239-260](../../../../../packages/opencode-plugin/src/tools/eidnara-search/execute.ts#L151)
  separates pre-pack ranking from delivered results. The
  [search tests, lines 218-255, 473-493](../../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts#L218)
  assert memory text, rationale, and packing behavior using a fake kernel.
- [Hint rendering, lines 9233-9295](../../../../../crates/daemon/src/transform.rs#L9233)
  compresses snippets and offers a project-memory search for their topic.
  That wording is a lead to test, not evidence that the registered tool
  supplies the source.

Reachability is `default-production`: the enabled plugin registers this path
without an experimental flag, as the configuration and registry show.
The claim is not that every query recovers every memory. The selected
discoverable scenario must construct a usable visible cue and returned result.

## Failure scenario

An evaluator passes a private `mem_...` ID to a helper and calls the obligation
discoverable. The ID never appeared in the consumer's context. Or a lexical
query finds the intended row before packing but its supporting text never
reaches the consumer. Both can pass an isolated search test while leaving
the actual omission scenario unrecoverable.

The competing explanation is a valid user-visible query. Retain a provenance
trail for each query operand or ID to captured context or earlier visible
tool output. A fixture may seed data privately; it may not supply hidden
retrieval arguments on the consumer's behalf.

## Timing windows and dependencies

Capture the invocation lacking the obligation, the registered tool call,
the kernel method, the returned tool text, and its consumer-visible delivery.
The case declares finite call and output limits before replay. Count fallback
and chunked reads separately from model-tool calls; do not silently enlarge
either budget to obtain a hit. Recovery is a bounded situation witness, not
an eventual-liveness promise.

## What a test must construct

1. Qualify the shared OpenCode invocation fixture. Use delivery's omission
   setup while retaining a legitimate visible query cue.
2. Reuse its independently admitted, project-visible memory whose authored
   text carries the obligation. Do not auto-promote session facts to pass.
3. Invoke the registered tool with visible-derived arguments and capture the
   actual `kernel.read` transport and final delivered text within limits.
4. Require at least one complete recovery situation, not merely search entry.
   A correct system must also produce this positive witness.
5. Keep the result labeled decision/rationale recovery. Pair it with a no-hit
   or undelivered control and the unavailable-credit property.

## Investigation log

### Q: Is the full invocation fixture qualified for this join?

- Sources examined: the route above, registry/search tests, and the plan's
  U3 qualification and capture requirements at lines 218-227.
- Findings: the registered memory path exists. Its unit witnesses do not
  capture an omission scenario, visible argument provenance, or provider
  delivery. The plan treats nonempty fold publication as a prerequisite.
- Missing evidence: a revision-bound, qualified invocation and tool-result
  capture for the declared scenario. No such replay ran in this task.
- Conclusion: unresolved, needs fixture qualification and captured delivery;
  lower-level helper success cannot close the reachability gap.

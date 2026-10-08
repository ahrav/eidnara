# Recovery: fault and situation map

Repository: `/local/home/ahrav/scratch/eidnara`.
Inspected revision: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
Date: 2026-09-19. All mapped situations remain unexercised in this task.

The [settled contract](https://github.com/ahrav/eidnara/issues/707),
"Requirements", "Six acceptance cases", and "Implementation Decisions",
is the contract lead for R3/R4/R10, C6, and KTD4. The supplied evidence inputs
include local code, docs, and history; no additional incidents or related
repositories were supplied. Code and existing tests were
inspected at the revision above, not the plan's `1555f00c`. Old daemon
portfolio CI and inventory claims remain stale leads. See the
[check inventory](existing-checks.md) for verified locations and unaudited
status, and the [catalog](catalog.md) for exact property predicates.

Availability below means a construction mechanism was found, not that a
campaign ran or that the whole fidelity join exists. Many failures here are
semantic substitutions or capability mismatches, not machine faults.

`cf-unavailable-evidence-no-credit` owns the single disposition predicate:
exactly one visible/discoverable/unavailable result per declared material
obligation and follow-up/scenario, with unavailable receiving no recovery or
useful-preservation credit even when abstention is safe. Delivery owns pressure
setup and consumes this predicate. The memory-recovery witness shares
delivery's [admission fixture](../delivery/catalog.md#cf-memory-credit-requires-admitted-visible-content),
not a second admission/omission fixture or disposition oracle.

## Fault classes and availability

| Class or situation | Construction availability | Boundary and limit |
| --- | --- | --- |
| Clean close/reopen | Existing daemon native-source witness at `crates/daemon/tests/harness_sources.rs:1419-1447`. | Reopen the same local store after successful publication. It is not SIGKILL, power loss, or a claim about disk barriers. |
| Same-length revision substitution | Historical selection at the original commit sequence is code-supported by `source_descriptor.rs:556-641`; the combined C6 execution is missing. | Publish original and same-lineage message successor before reopen, then recover the original. Changing bytes at the same timestamp is a publication conflict, not this control. |
| Original revision after succession | `slice/write.rs:433-477` and `object_write.rs:90-118` invalidate the descriptor observation, not its distinct evidence metadata. | Save the original descriptor commit sequence and binding; select with `live_source_descriptors`, then use its evidence/digest pair with `read_artifact`. No direct CAS-file bypass. |
| Old hold after reopen | `source_hold.rs:1045-1078` rejects an old epoch/binding; reopening increments the lease epoch. | Do not reuse an old hold or cursor. Historical descriptor selection at the saved sequence does not require a hold. |
| Different occurrence, identical bytes | Existing native fixture creates distinct occurrences at `harness_sources.rs:1283-1310`. | Select by complete native binding, not content or digest. The negative control supplies a valid alternate occurrence. |
| Normalized/full-summary substitution | Chunk cleanup/normalization at `history_summarizer_chunk.rs:1187-1204` and compaction at `:188-206` provide the distinction. | Present a normalized transcript or fuller summary to the comparison as its real representation. Do not mislabel it native in setup. |
| Refused exact source | Existing scanner-refusal fixture at `harness_sources.rs:1459-1483`. | Use synthetic bytes; retain typed refusal and content-free diagnostics. Never weaken the scanner to achieve exact recovery. |
| Purged or retired original evidence | `cas/read.rs:60-85` requires live evidence metadata and no purge tombstone. | Historical descriptor availability does not bypass the current artifact-read guard. A real refusal remains unavailable with zero credit; descriptor succession alone is not such a refusal. |
| Missing/corrupt native artifact | Kernel test seam exists at `kernel_cas.rs:392-431`; joining it to C6 is missing. | Inject only into a disposable fixture. A positive reopen row must not silently become a refusal row. |
| Absent versus empty text | `SourceRow.text` is optional at `source_export.rs:84-101`; the inventory helper folds absence to empty at `harness_sources.rs:731-744`. | Observe present text before byte comparison. `Some("")` can be valid; `None` is not exact recovery of an empty block. |
| Unavailable or disabled memory read | Fake-kernel/transport controls exist in search `tools.test.ts:350-425`. | Full invocation behavior still needs a registered-tool capture. No-consumer fallback may deliver evidence; busy/stale/disabled outcomes cannot be presumed equivalent. |
| Empty or packed-out memory result | Search tests at `tools.test.ts:473-508` distinguish `prePack`, `delivered`, and empty results. | Presence before packing is not delivery. Apply the case's call/output limits to the actual returned text. |
| Visible cue versus private ID | Memory query/schema exists; end-to-end argument provenance is missing. | Use only captured context or prior visible tool output for consumer arguments. Fixture-private IDs can validate results, not generate calls. |
| Same-name facade substitution | Both paths exist; registry/client trace differs from `lib.rs:11887-11945`. | Direct history/note facade execution is not the registered OpenCode memory search witness. |
| Unsupported expansion / Pi transform | Registry and Pi entrypoint assertions exist; the Pi flag is now `true`. | Unsupported expansion is an observed capability state, not a successful recovery. Pi folding is outside the corpus and earns no Pi tier scenario. |
| Capability drift | Full registry/schema comparison and Pi flag provide change detection. | A changed reachable tool, search source, or Pi gate invalidates the pinned expectation for review; do not force the old limitation to remain true. |

## Per-property enabling state

| Property | Required construction | Observe at the highest existing boundary | Excluded conclusion |
| --- | --- | --- | --- |
| cf-native-reopen-bytes | Original publication sequence/binding, same-length message successor before reopen, historical original selection and verified read yielding `text = Some(text)`. | Extend the daemon native-source witness with `live_source_descriptors` at the saved sequence and `read_artifact` on the selected original handle. | No old-hold reuse, direct CAS-file bypass, successor/distinct-occurrence substitution, agent access, crash, or power-loss guarantee. |
| cf-exact-source-binding | Equal-length distinct revisions, equal-text distinct occurrences, normalized/full-summary substitutes, and a native positive control. | C6 comparison over source observations from that daemon witness. | A valid alternate digest does not prove the requested occurrence/revision. |
| cf-unavailable-evidence-no-credit | Visible/discoverable controls and unavailable cases with refused/no-hit/undelivered evidence; delivery supplies pressure setup. | One disposition predicate over native outcome and captured OpenCode invocation/tool-result observations. | No missing/duplicate dispositions; safe abstention cannot clear preservation failure; hidden storage and completed-empty search are not recovery. |
| cf-visible-memory-recovery | Shared delivery admission/invocation fixture, qualified omission, legitimate visible cue, enabled bound plugin, and declared finite budgets. | Registered OpenCode tool execution and actual consumer-visible result. | No duplicate admission fixture, memory auto-promotion, private-ID shortcut, or exact transcript claim. |
| cf-shipped-recovery-capabilities | Captured enabled/disabled/compaction-off registry inputs, expected schema, execution trace, and Pi gate at the evaluated revision. | Test-only report predicate consuming production registry/entrypoint evidence. | No claim of automatic product reporting, direct-facade substitution, synthetic Pi tiers, or permanent ban on future capability. |

Keep a successful internal native read and unavailable consumer expansion in
the same scenario when both are true. They are not contradictory results.
Keep decision/rationale memory recovery separate from raw-source exactness.

The selector accepts both `Messages` and `RawToolSpans`. Use a message revision
for same-lineage succession: tool `result_revision` is itself a native
identity field, so changing it constructs a different lineage. Preserve the
original tool binding for the exact tool-byte control; it does not replace
the original-message-after-succession control. Verified references and the
rejected impossibility claim are in [native evidence](evidence/cf-native-reopen-bytes.md).

## Coverage checks to add

These are fixed proposed marker names, not installed assertions. They assert
constructible situations, never the negation of the safety property.
The named `sometimes` recovery record additionally requires its full delivered
outcome. No marker is evidence until the corresponding scenario runs.

| Marker | Semantics | Required observation |
| --- | --- | --- |
| `cf-recovery-native-reopened` | sometimes | Original and same-length successor publications complete before clean close/reopen; the saved original commit sequence and binding remain the selection target. |
| `cf-recovery-equal-byte-length-revisions` | sometimes | Two accepted native revisions with equal UTF-8 byte lengths and different byte arrays exist before the original-revision comparison. |
| `cf-recovery-equal-text-occurrences` | sometimes | Two distinct complete native occurrence bindings have identical text before candidate selection. |
| `cf-recovery-normalized-alternative` | sometimes | The comparison receives a normalized transcript candidate alongside an independently stored native source. |
| `cf-recovery-refused-native-attempt` | sometimes | A native exact publication is attempted with synthetic scanner-refused bytes, and its typed outcome is recorded. |
| `cf-recovery-absent-obligation` | sometimes | The complete pre-recovery invocation lacks the annotated material obligation, not merely one tier. |
| `cf-recovery-present-empty-source` | sometimes | A retained, selected native empty block is observed with present text, separately from an absent-text observation. |
| `cf-recovery-memory-packed-out` | sometimes | The intended memory is a pre-pack hit but is absent from delivered text; the report still evaluates the omission. |
| `cf-recovery-visible-query-issued` | sometimes | A registered call uses arguments traced to a captured visible cue and reaches a kernel read within declared limits. |
| `cf-recovery-registered-kernel-read` | reachable | The actual registered callback traverses `executeEidnaraSearch` to the client's `kernel.read` send point. |
| `cf-recovery-pi-gate-observed` | reachable | The Pi entrypoint's transform-availability/compaction gate is evaluated. This is not history-transformation coverage. |

For each negative control, also require the comparison to be evaluated and a
valid native or delivered-memory positive control. A missing marker may mean
the generator missed its situation or the previously reachable state changed.
Investigate that distinction; do not skip the case or infer formal liveness.

## Cheapest valid oracle and handoff order

| Rank | Records | Lowest-cost valid observation | Remaining join |
| --- | --- | --- | --- |
| 1 | cf-shipped-recovery-capabilities | Reuse complete registry/schema and Pi entrypoint checks, with the callback/client trace. | Bind the inventory and unsupported report to the evaluated build/configuration. |
| 2 | cf-exact-source-binding | Compare fixed native annotations with valid alternate source observations and a matched control. | Store complete expected and observed bindings without using generated text as the oracle. |
| 3 | cf-native-reopen-bytes | Extend the daemon witness with historical descriptor selection and verified artifact read after reopen. | Execute the code-supported sequence with both revisions published before reopen and compare the original binding/bytes. |
| 4 | cf-unavailable-evidence-no-credit | Reuse typed outcomes and delivery's pressure observations in one disposition predicate. | Require one visible/discoverable/unavailable result per tuple; retain useful-preservation failure independently of consumer safety. |
| 5 | cf-visible-memory-recovery | Reuse delivery's admission/invocation fixture for bounded registered search delivery. | Add visible argument provenance and actual result delivery to the shared observations. |

This ranking is an observation-boundary recommendation, not a test-form
decision. `/testing:test-strategy` owns the form and oracle implementation.
Use the existing deterministic fixtures first. A simulation harness is not
required merely because the topic is recovery; if temporal fault exploration
is later selected, route its construction to
`/testing:deterministic-simulation-testing`.

## Constraints and unresolved prerequisites

- Qualification of the OpenCode fold/invocation fixture is unresolved here.
  A skipped, gated, empty-capture, or raw-pass-through row receives no
  compression/recovery coverage. Preserve captures before helper resets.
- Cases must fix call/output limits before replay. Count actual tool calls,
  underlying fallback/chunked reads, and returned output separately; do not
  invent a universal production recovery deadline.
- Original-revision constructibility is supported by the existing historical
  descriptor selector and still-live evidence handle. The combined execution
  remains unexercised. Use no old source hold after reopen and do not replace
  the original with a successor or distinct occurrence to pass. Purge/refusal
  controls retain unavailable-zero-credit treatment. No source-access API,
  retention relaxation, second memory store, admission bypass, or Pi transform
  is authorized.
- Human-reviewed obligations decide whether abstention is allowed and whether
  delivered memory conveys the intended decision/rationale. No model call or
  semantic acceptance result is produced by this catalog.
- Independent portfolio evaluation follows. No evaluation file, source/test
  edit, CI edit, tracker write, or git mutation is part of this work.

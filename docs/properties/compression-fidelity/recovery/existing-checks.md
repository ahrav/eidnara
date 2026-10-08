# Recovery: existing checks

Repository: `/local/home/ahrav/scratch/eidnara`.
Inspected revision: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
Date: 2026-09-19. No tests or campaigns ran for this inventory.

The [settled contract](https://github.com/ahrav/eidnara/issues/707),
"Requirements", "Six acceptance cases", and "Implementation Decisions",
provides R3/R4/R10, C6, and KTD4 as claim leads. Local source and test bodies
provide the check evidence below. These are the supplied evidence inputs;
no additional incidents or related repositories were supplied. The older
daemon portfolio's inventory and CI claims are stale leads; they are not
copied into this inventory. The plan's `1555f00c`
inspection is not the revision inspected here.

Every check listed here is **unaudited**. A location and an assertion do not
prove non-vacuity or complete fidelity. Test adequacy belongs to
`/testing:invariant-test-review`; production guard adequacy belongs to
`/low-level-systems:defensive-assertions-and-invariant-guards`. There is no CI
execution claim. [Catalog records](catalog.md) remain `Exercised: not yet`.

## Native source checks

The highest existing boundary is
[`crates/daemon/tests/harness_sources.rs`](../../../../crates/daemon/tests/harness_sources.rs).
Its kernel, native adapters, publisher, and export path are real. Extend its
exactness witness rather than duplicating source retention or receipt tests.

| Location | Semantics and assertion/message | Relevant record | Status |
| --- | --- | --- | --- |
| [harness_sources.rs:442-728](../../../../crates/daemon/tests/harness_sources.rs#L442) | `adapters_bind_native_identity_exactly_and_refuse_missing_identity` checks adapter results and refusal variants for native records. | cf-native-reopen-bytes, cf-exact-source-binding | unaudited |
| [harness_sources.rs:1283-1310](../../../../crates/daemon/tests/harness_sources.rs#L1283) | Three equal-text native inputs produce distinct occurrence IDs; message: `equal bytes, three native occurrences`. | cf-exact-source-binding | unaudited |
| [harness_sources.rs:1313-1335](../../../../crates/daemon/tests/harness_sources.rs#L1313) | Replay keeps receipt/occurrence and tip; changed bytes at the same identity/revision yield `IdentityReused`. These are publication checks, not source-selection credit. | cf-exact-source-binding | unaudited |
| [harness_sources.rs:1359-1417](../../../../crates/daemon/tests/harness_sources.rs#L1359) | Newer revision replaces its predecessor; stale publication is refused and its retained evidence is retired. Repeated stale publication checks `EvidenceMissing`. | cf-exact-source-binding, cf-unavailable-evidence-no-credit | unaudited |
| [harness_sources.rs:1419-1447](../../../../crates/daemon/tests/harness_sources.rs#L1419) | Checks exact tool output and a retained empty message, then cleanly reopens and compares inventory; messages include `the tool bytes are exact` and `reopen preserves every byte and identity`. | cf-native-reopen-bytes | unaudited |
| [harness_sources.rs:1459-1483](../../../../crates/daemon/tests/harness_sources.rs#L1459) | A synthetic credential-shaped output yields `ExactBytesRewritten`, no inventory, and unchanged tip; formatted error/unit output must not contain the secret. | cf-unavailable-evidence-no-credit | unaudited |

The inventory helper at
[lines 731-744](../../../../crates/daemon/tests/harness_sources.rs#L731)
keeps `(class, text, occurrence_id)` and uses `unwrap_or_default()` for text.
It does not separately compare every field in the exported row. C6 requires
an independent native-revision binding and present-text observation, not
only this set equality. This describes the checked values, not an adequacy
verdict on the existing test's original claim.

### Supporting kernel checks

These are lower-level leads to reuse, not separate new retention properties.

| Location | Semantics and assertion/message | Relevant record | Status |
| --- | --- | --- | --- |
| [kernel_exact_artifacts.rs:142-173](../../../../crates/kernel/tests/kernel_exact_artifacts.rs#L142) | Exact retention returns the independently hashed fixture bytes; reads and object-file bytes equal input. Equal content can share an object across evidence rows. | cf-native-reopen-bytes, cf-exact-source-binding | unaudited |
| [kernel_exact_artifacts.rs:177-241](../../../../crates/kernel/tests/kernel_exact_artifacts.rs#L177) | Rewrite/scan-limit refusals occur before staging; checks no new objects, evidence, or commits. The refusal message does not include payload content. | cf-unavailable-evidence-no-credit | unaudited |
| [kernel_exact_artifacts.rs:361-385](../../../../crates/kernel/tests/kernel_exact_artifacts.rs#L361) | Clean reopen and receipt replay return the same handle and bytes; different bytes with reused intent yield `OperationKeyReused`. | cf-native-reopen-bytes | unaudited |
| [kernel_cas.rs:392-431](../../../../crates/kernel/tests/kernel_cas.rs#L392) | Missing/FIFO-swapped objects yield `MissingObject`; overwritten or oversized objects yield `CorruptObject`. | cf-unavailable-evidence-no-credit | unaudited |
| [kernel_cas.rs:435-448](../../../../crates/kernel/tests/kernel_cas.rs#L435) | Malformed digest reads yield `InvalidInput`. | cf-unavailable-evidence-no-credit | unaudited |

## Registered OpenCode checks

| Location | Semantics and assertion/message | Relevant record | Status |
| --- | --- | --- | --- |
| [tool-registry.test.ts:69-83](../../../../packages/opencode-plugin/src/plugin/tool-registry.test.ts#L69) | Checks exactly the four registered IDs, absence of `ctx_expand`, empty registry when disabled, and search registration despite disabled memory configuration. | cf-shipped-recovery-capabilities | unaudited |
| [tool-registry.test.ts:85-119](../../../../packages/opencode-plugin/src/plugin/tool-registry.test.ts#L85) | Checks advertised fields; search exposes `query`, `limit`, `sources`. | cf-shipped-recovery-capabilities | unaudited |
| [tool-registry.test.ts:127-164](../../../../packages/opencode-plugin/src/plugin/tool-registry.test.ts#L127) | Compaction-off removes only reduce; default and explicit compaction-on sets agree. | cf-shipped-recovery-capabilities | unaudited |
| [tools.test.ts:202-232](../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts#L202) | Omitted sources and `["memory"]` agree; a text query returns the matching memory, not the distractor; read is `explicit_search`, gated. | cf-visible-memory-recovery | unaudited |
| [tools.test.ts:235-255](../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts#L235) | Rejected-approach output includes warning, safer alternative, memory ID, and `Rationale: Redis needs a network hop.` | cf-visible-memory-recovery | unaudited |
| [tools.test.ts:257-308](../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts#L257) | Truncated memory reads carry a note; object-ID queries use filtered reads and split a truncated multi-ID request. `match=exact` denotes ID matching. | cf-visible-memory-recovery, cf-unavailable-evidence-no-credit | unaudited |
| [tools.test.ts:350-366](../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts#L350) | A stale source returns the explicit projector-lag error rather than ranked memory rows. | cf-unavailable-evidence-no-credit | unaudited |
| [tools.test.ts:369-405](../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts#L369) | No-consumer refusal permits an ungated read with a canonical-tip note; `store_busy` stays an error after one read. | cf-unavailable-evidence-no-credit | unaudited |
| [tools.test.ts:407-425](../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts#L407) | Disabled and absent-daemon clients produce explicit errors and no transport calls. | cf-unavailable-evidence-no-credit | unaudited |
| [tools.test.ts:428-439](../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts#L428) | Full/light descriptions name memory and `mem_<32hex>`, not expansion, compacted history, commits, or notes. | cf-shipped-recovery-capabilities | unaudited |
| [tools.test.ts:444-471](../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts#L444) | Blank queries are invalid; a direct-ID helper result and wrapper text agree, with one delivered result. | cf-visible-memory-recovery | unaudited |
| [tools.test.ts:473-508](../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts#L473) | Packed-out hits remain in `prePack`, not `delivered` or returned text; a complete empty search has `empty-results`. | cf-unavailable-evidence-no-credit | unaudited |
| [tools.test.ts:510-525](../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts#L510) | Returned token accounting includes the truncation note. | cf-visible-memory-recovery | unaudited |

Search tests use a fake kernel/transport. Registry tests enumerate factories;
search tests call the search wrapper/helper. Neither observation alone is
the combined invocation -> visible arguments -> registered tool -> real
`kernel.read` -> consumer-visible result witness.

The memory-recovery witness shares delivery's
[admission fixture](../delivery/catalog.md#cf-memory-credit-requires-admitted-visible-content).
Delivery supplies admission/inclusion and pressure observations; recovery adds
the explicit tool-call/result join. Capability reporting is a test-only
predicate over these registry inputs, not automatic product reporting.

## Pi entrypoint check

| Location | Semantics and assertion/message | Relevant record | Status |
| --- | --- | --- | --- |
| [index.test.ts:59-78](../../../../packages/pi-plugin/src/index.test.ts#L59) | Requires `PI_TRANSFORM_AVAILABLE` true; an acknowledged eviction answers compaction and a missing one cancels. This changed after the inspected revision, so the Pi-unsupported pin needs review. | cf-shipped-recovery-capabilities | unaudited |

## Production guards and type boundaries

| Location | Existing check | Status |
| --- | --- | --- |
| [harness_sources.rs:11-14](../../../../crates/daemon/src/harness_sources.rs#L11) | Const assertion requires two kernel harness spellings. This is namespace agreement, not fidelity. | unaudited |
| [cas/read.rs:50-112](../../../../crates/kernel/src/cas/read.rs#L50) | Runtime checks return typed errors for malformed handles, non-live references, tombstones, missing objects, and digest mismatch. | unaudited |
| [source_export.rs:535-566](../../../../crates/kernel/src/source_export.rs#L535) | Runtime preflight checks descriptor version, revision, evidence/digest association, identity, span, and policy. | unaudited |
| [source_export.rs:372-403](../../../../crates/kernel/src/source_export.rs#L372) | Runtime materialization checks verified bytes, byte length, UTF-8 span, and selected payload identity. | unaudited |
| [execute.ts:33-86,122-175](../../../../packages/opencode-plugin/src/tools/eidnara-search/execute.ts#L33) | Memory-only source validation, query checks, project binding, and explicit empty-source behavior bound the allowed search request. | unaudited |
| [execute.ts:230-252](../../../../packages/opencode-plugin/src/tools/eidnara-search/execute.ts#L230) | Nonavailable states, the narrow fallback, and truncation/unresolved-ID notes remain distinct. | unaudited |
| [Pi index.ts:89,367-370](../../../../packages/pi-plugin/src/index.ts#L367) | Transform availability, now `true`, combines with the requested setting to decide whether compaction is off. | unaudited |

No production assertion that measures compression fidelity or grants exact
consumer-recovery credit was found in these boundaries. Representation and
error guards are supporting facts, not a runtime semantic oracle.

## None found and quiet areas

`crates/daemon/testdata/compression-fidelity.json`, its Rust module, and
`docs/compression-fidelity.md` arrive with U1 (#718);
`packages/e2e-tests/tests/rust-compression-fidelity.test.ts` is still absent. Existing
checks inventoried here remain supporting evidence, not an implemented corpus.

- **C6 full join:** none found combining original native occurrence/revision,
  equal-length alternatives, clean reopen, selected bytes, and consumer
  capability disposition.
- **Substitution rejection:** none found granting/denying fidelity credit for
  a valid wrong occurrence, a same-length wrong revision, or a normalized
  transcript. Publication conflict tests answer a different question.
- **Argument discoverability:** none found deriving the registered call's
  arguments solely from the captured invocation or prior visible tool output.
- **Disposition accounting:** none found assigning exactly one of visible,
  discoverable, or unavailable to every material-obligation/follow-up/scenario
  tuple while retaining useful-preservation failure despite safe abstention.
  `cf-unavailable-evidence-no-credit` owns that predicate, including zero
  recovery credit for empty results or internal reads alone; delivery consumes
  it rather than duplicating it.
- **Consumer exact expansion and Pi tiers:** consumer exact expansion is
  unsupported at the current capability boundary. Pi folds history since
  `4ba36413a`, but no fidelity scenario replays Pi, so Pi tiers have no
  coverage here. A capability change
  must fail the pinned unsupported expectation and prompt review.
- **Abrupt crash or power loss:** none claimed by these clean-reopen witnesses.
  No durability conclusion or new fault campaign is inferred.

These are scoped search results, not a repository-wide assertion that no
other checks exist. Source bodies and line numbers were inspected at the
named revision; runtime execution, CI selection, and adequacy remain open.

# Compression fidelity: recovery properties

Repository: `/local/home/ahrav/scratch/eidnara`.
Inspected revision: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
Date: 2026-09-19. These are claims to test, not execution or proof results.

## Scope and authority

This part owns R3, R4, R10, C6, and KTD4 from the supplied, settled
[Compression Fidelity Contract](https://github.com/ahrav/eidnara/issues/707).
See "Requirements", "Materiality and minimum meaning by tier", "Six acceptance
cases", "Implementation Decisions", and "Source and evidence flow".
The plan records inspection at `1555f00c`; references here use the checkout
at `99f68bd3`. The plan supplies the intended fidelity contract, not evidence
that its replay corpus or capability checks are implemented.
Plan line numbers remain historical provenance, not issue offsets. The
[catalog index](../README.md#scope-and-provenance) preserves the original
plan's title, date, SHA-256, and the published specification's readback evidence.

Evidence supplied for this task comprises the plan, local repository, docs,
and local history. No additional incidents or related repositories were
supplied; this records the inputs, not a user-imposed restriction on evidence.
Local history yielded no exhibited fidelity failure promoted here.
The existing [daemon history portfolio](../../daemon/history_summarizer/catalog.md)
is a lead only. Its CI, inventory, and provenance claims are pre-evaluated as
stale; none is inherited as coverage. Native exact-source fidelity has no
existing catalog owner in the supplied scope.

[METHOD](../../METHOD.md) governs records. The
[host wire contract](../../../host-wire-protocol.md) remains normative.
This catalog changes no wire names or literals. It neither adds a native
write endpoint nor recatalogs receipt, retention, source-hold, or CAS policy.
It owns the joins between native identity, returned bytes, visible evidence,
and reachable tools. It does not promise an expansion API or Pi transform.

## Boundaries and model

| Boundary | Observed mechanism | What this part may claim |
| --- | --- | --- |
| Native-source witness | `opencode_units` / `pi_units` produce identity-bound blocks; `SourcePublisher` retains their exact strings; source export verifies artifacts. | A daemon integration witness can compare selected native bytes after clean reopen. It is privileged, not an agent tool. |
| Summary input and transcript | Chunk assembly cleans, normalizes, joins, and compacts text; publication stores the supplied chunk transcript. | Neither a fuller summary nor a normalized transcript establishes native exactness. |
| OpenCode recovery | Registry -> search wrapper -> `executeEidnaraSearch` -> kernel client -> `kernel.read` -> memory decision ranking and packing. | A delivered memory can recover a decision or rationale. It is not exact transcript recovery. |
| Same-name daemon facade | `handle_eidnara_search_facade` searches history segments and notes. | Calling this facade directly does not exercise the registered OpenCode search tool. |
| Pi | `PI_TRANSFORM_AVAILABLE` was false at the inspected revision; since `4ba36413a` (#850) it is true and the Pi `context` handler folds history. | The corpus replays OpenCode only, so this catalog grants no Pi P1-P5 coverage. Native Pi source parsing is a different capability. |

The highest existing witnesses are the daemon native-source integration test,
the OpenCode registry/search tests, and the Pi entrypoint test. A complete
visible-recovery join belongs at the existing OpenCode invocation boundary,
not at an invented storage API. Lower-level checks support those witnesses;
they do not replace them. Exact native bytes mean UTF-8 bytes of the selected
decoded text/tool-output string, not JSON escape spelling or the whole
session file.

The existing original-revision selector is code-supported: retain the commit
sequence after original descriptor publication and its native binding, publish
the same-length successor, then cleanly reopen. Call
`live_source_descriptors` for the original class and saved sequence, match the
complete binding, and call `read_artifact` with that descriptor's evidence
handle. Succession invalidates the descriptor observation, not its evidence
metadata. The new store epoch forbids reusing an old source hold. This supports
constructing C6 with both publications before reopen; the combined execution
witness is still missing. See [native evidence](evidence/cf-native-reopen-bytes.md).

### Lens summary

System passes covered architecture/data flow, persistence, concurrency,
safety claims, bounded recovery claims, history, existing tests, degradation,
dependencies, product impact, and unproven assumptions. Property passes
covered integrity, concurrency, recovery, protocol, resources, security,
lifecycle, replay, and version compatibility. Distributed coordination is
outside this local-store/client join. Both wildcard passes ran last.

The passes preserve five distinct findings: byte persistence needs native
identity; valid bytes can belong to the wrong source; refusal is not recovery;
memory recovery needs visible arguments and delivered results; capability
reports must match the shipped registry and Pi gate. Wildcard findings add
two cautions: `None` source text is not `Some("")`, and memory `match=exact`
means object-id matching, not exact-source fidelity. Repeated discovery is
not independent corroboration. The non-durable method-note path is
`/tmp/opencode/compression-fidelity-recovery-lenses.md`. The summaries here
retain the findings without requiring that temporary file.

## Index

| Property | Requirement | Type | Reachability | Semantics |
| --- | --- | --- | --- | --- |
| [cf-native-reopen-bytes](#cf-native-reopen-bytes) | R4, C6 | safety | test-only | always |
| [cf-exact-source-binding](#cf-exact-source-binding) | R4, C6 | safety | test-only | always |
| [cf-unavailable-evidence-no-credit](#cf-unavailable-evidence-no-credit) | R3, R4 | safety | test-only | always |
| [cf-visible-memory-recovery](#cf-visible-memory-recovery) | R3, KTD4 | reachability | default-production | sometimes |
| [cf-shipped-recovery-capabilities](#cf-shipped-recovery-capabilities) | R3, R4, R10 | safety | test-only | always |

Reachability is verified separately in each evidence file. `test-only` names
the privileged C6 witness, replay accounting, or capability-report predicate,
not a claim that the underlying storage and registry paths are test-only.
`default-production` names the enabled plugin's existing memory-search path;
it does not mean the required scenario has been exercised. Four records are
test-only and one is default-production. All five are active; U2 partially exercises cf-native-reopen-bytes and
cf-exact-source-binding, and three are unexercised.
Semantics totals: four `always`, one `sometimes`; no unbounded liveness claim.

## Oracle vocabulary

The following notation describes observations for a test, not a production
schema. An expected source binding comes from independent native fixture
annotations: class, project, harness, durable session, native message/block
or parent/tool-call/result identity, revision, representation, and selected
UTF-8 byte span. Preserve the descriptor-to-evidence/digest association, but
never substitute a digest or equal length for that binding.

[cf-unavailable-evidence-no-credit](#cf-unavailable-evidence-no-credit) is the
canonical owner of the three-way disposition and unavailable-zero-credit
predicate. Each declared material-obligation/follow-up/scenario tuple has
exactly one disposition. Delivery consumes this predicate and owns pressure
setup; it does not define a second disposition oracle.

Inspect the complete captured invocation and returned tool results. Internal
exact-source success, consumer recovery, useful preservation, and consumer
safety remain separate outcomes. A search result in `prePack` but not in
delivered text is not visible evidence. Private oracle IDs and source bytes
may validate a result, but may not generate consumer arguments.

### cf-native-reopen-bytes

Type: safety
Reachability: test-only
Status: active
Exercised: partial - `crates/daemon/tests/harness_sources.rs` reads C6's original
revision byte for byte after a same-length successor and clean reopen (U2,
#719). No power-loss or agent-access claim follows.
Guarantee: A successfully retained and still-readable native source returns
the selected original UTF-8 bytes after clean reopen.
Check: `always` - for every positive C6 selection, require a successful read
with `text = Some(text)`, the independently expected original source binding,
and UTF-8 byte-array equality with the annotated native slice; check every
positive selection rather than allowing a failed read or `None` text to make
the comparison vacuous.
Fault/timing angle: Publish the original and its same-length successor before
clean close/reopen, then select at the original publication sequence rather
than the current tip.
Required faults and enabling state: Publish safe native text and settled tool
output containing CRLF, whitespace, Unicode, a command, and a numeric value;
retain the original descriptor commit sequence and binding, publish a
same-lineage message successor, and reopen without purge or evidence
retirement. Use historical descriptor selection and the original evidence
handle, not an old hold, current live inventory, or direct object-file read.
Confidence: high - [evidence](evidence/cf-native-reopen-bytes.md). Adapter,
descriptor/evidence succession, historical selection, epoch fencing, artifact
read, and existing reopen assertions were inspected. This is code-derived
constructibility, not a passing combined C6 execution.
Existing check: `crates/daemon/tests/harness_sources.rs:1283-1454` compares
native occurrences and export inventory after reopen; unaudited. See
[native checks](existing-checks.md#native-source-checks).
Impact: An exact command, value, or quoted tool output can silently change
while a summary or equal-length artifact appears intact.
Open questions:
- Does the combined C6 witness execute both publications before reopen, then
  select the original at its saved sequence and recover its exact bytes? The
  existing selector is code-supported; that joined execution remains needed.

### cf-exact-source-binding

Type: safety
Reachability: test-only
Status: active
Exercised: partial - `crates/daemon/tests/harness_sources.rs` checks the
wrong revision, an equal-text other occurrence, a normalized substitute,
absent versus empty, and deleted evidence for C6 (U2, #719).
Guarantee: Exact-source credit requires the annotated native occurrence,
revision, representation, span, and bytes rather than a plausible substitute.
Check: `always` - an observation receives exact-source credit only if its
complete source binding equals the independent expected binding, its verified
selected bytes equal the native slice, and its representation is native;
evaluate every substitution even when length, text, or digest happens to match.
Fault/timing angle: Substitute evidence after selection, including a valid
artifact from a different occurrence or a newer revision of equal byte length.
Required faults and enabling state: Construct different revisions with equal
UTF-8 byte lengths, distinct occurrences with identical text, a normalized
transcript, and a fuller summary; preserve a valid native positive control.
Confidence: high - [evidence](evidence/cf-exact-source-binding.md). Native
identity fields, artifact verification, export validation, and normalization
were inspected; the proposed fidelity predicate is not implemented evidence.
Existing check: `crates/daemon/tests/harness_sources.rs:1303-1335` checks
distinct occurrences and publication conflict; unaudited. No joined C6
substitution-credit check was found.
Impact: A valid artifact can be misattributed to the requested event, and a
normalized command can be reported as an exact quotation.
Open questions:
- How will the replay observation retain the native binding separately from
  the summary/citation aliases? The corpus and observation owner are planned,
  not an implemented format established by this inspection.

### cf-unavailable-evidence-no-credit

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - unavailable/refused reads have not been joined to a
captured omission scenario and its separate consumer-safety outcome.
Guarantee: Every material-obligation/follow-up/scenario tuple has exactly one
evidence disposition, and unavailable evidence receives no recovery or useful
preservation credit even when the consumer abstains safely.
Check: `always` - require a captured invocation and exactly one disposition
for every declared tuple: visible if the complete pre-recovery invocation
includes the obligation with its qualifiers; otherwise discoverable if a
permitted registered-tool replay
using visible-derived arguments delivers it within the declared call/output
budget; otherwise unavailable. For unavailable, require recovery and useful
preservation credit false and retain the preservation failure independently
of safe abstention. Empty results and internal reads alone never earn recovery
credit. This total, exclusive accounting must hold for every evaluated tuple.
Fault/timing angle: Refusal before native retention, unavailable artifact at
read, unavailable search, or removal of the only delivered evidence.
Required faults and enabling state: Construct visible and discoverable
controls, then remove the obligation from history, live tail, included memory,
and relevant hints for unavailable cases; include a refused native source,
a nonavailable read, a no-hit search, and a target omitted during packing.
Reuse delivery's pressure setup and evaluate this one disposition predicate.
Confidence: high - [evidence](evidence/cf-unavailable-evidence-no-credit.md).
Typed read failures, scanner refusal, and search outcome distinctions were
inspected; no consumer-safety or fidelity campaign ran.
Existing check: `crates/daemon/tests/harness_sources.rs:1459-1483` checks
refused bytes; search `tools.test.ts:350-425,473-508` checks unavailable and
undelivered results; all unaudited. No complete disposition join was found.
Impact: Safe abstention or a successful internal witness can conceal loss of
the evidence needed for a useful, supported answer.
Open questions:
- Which C6 scenarios explicitly permit abstention? The human-reviewed cases
  must establish this before scoring; unavailable still receives no recovery
  credit. (needs human input)

### cf-visible-memory-recovery

Type: reachability
Reachability: default-production
Status: active
Exercised: partial - `packages/e2e-tests/tests/compression-fidelity-memory.test.ts`
calls `eidnara_search` from a captured C3 pass lacking the memory, with query
terms taken from the visible request and sources `["memory"]`, and receives
the memory text in one call within 16 KiB (U3). Human review of the
delivered text is absent.
Guarantee: A declared discoverable decision or rationale has a bounded
registered-tool witness that starts from consumer-visible information and
delivers the supporting memory to that consumer.
Check: `sometimes` - in the declared recovery campaign, witness a captured
OpenCode invocation lacking the obligation, a query or memory ID traceable to
visible context or prior visible tool output, execution through the registered
search tool and `kernel.read`, and delivered text satisfying the annotated
decision/rationale within the case's fixed call/output budget; this is
situation coverage, not merely entry into a function or exact-source credit.
Fault/timing angle: Omit material history while preserving a usable visible
cue; distinguish matched memory candidates from returned, packed evidence.
Required faults and enabling state: Use an enabled plugin, a correctly bound
project, an independently admitted memory, an observable tool-result capture,
and finite declared limits; keep private fixture IDs out of consumer input
and include a matched unavailable or undelivered control. Share the admission
and invocation fixture with delivery's
[cf-memory-credit-requires-admitted-visible-content](../delivery/catalog.md#cf-memory-credit-requires-admitted-visible-content),
then apply the canonical disposition predicate rather than duplicating it.
Confidence: high - [evidence](evidence/cf-visible-memory-recovery.md). The
registry-to-client route and memory packing were inspected; full invocation
reachability and semantic adequacy remain unexercised.
Existing check: `packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts:218-255`
checks returned summaries and rejection rationale with a fake kernel;
unaudited. No visible-argument/provider-result join was found.
Impact: Stored memory or an attractive hint can be credited as recoverable
even though the consumer cannot formulate or complete the required call.
Open questions:
- Can the existing OpenCode invocation fixture qualify nonempty history
  publication and preserve both pre-call and tool-result captures for this
  scenario? The plan identifies qualification as a prerequisite.

### cf-shipped-recovery-capabilities

Type: safety
Reachability: test-only
Status: active
Exercised: partial - `packages/e2e-tests/src/compression-fidelity/capabilities.ts`
pins the four Eidnara tools, `eidnara_search` sources `["memory"]`, no
exact-expansion tool, and `PI_TRANSFORM_AVAILABLE`; the delivery campaign checks
each case's captured m1 request against the pin, and its unit test shows drift
for an added expansion tool, a widened source enum, a missing tool, and a
disabled Pi transform (U3, #720). The registered search route to `kernel.read`
is not yet replayed through a tool call.
Guarantee: Recovery and tier-coverage claims reflect shipped capabilities,
not privileged helpers, same-named daemon facades, or unsupported Pi transforms.
Check: `always` - compare each replay report with its captured registry inputs,
complete expected tool/argument/source surface for that configuration,
registered search route to `kernel.read`, and Pi transform gate; for this
revision report exact expansion and Pi P1-P5 transformation unsupported and
give neither consumer exact-recovery nor Pi tier-coverage credit. A changed
capability fails the pinned expectation for review. This checks a test report
against production evidence, not automatic product reporting behavior.
Fault/timing angle: A test substitutes direct facade/helper access, or shipped
capabilities change while the unsupported baseline remains unchanged.
Required faults and enabling state: Enumerate enabled, compaction-off, and
disabled registry outputs; inspect visible schemas and actual transport
method; evaluate Pi's existing compaction gate without synthetic tier replay.
Confidence: high - [evidence](evidence/cf-shipped-recovery-capabilities.md).
Registration, memory-only execution, default enablement, and Pi's transform flag,
false at the inspected revision, were inspected; no future capability is assumed.
Existing check: `packages/opencode-plugin/src/plugin/tool-registry.test.ts:69-119,127-164`
pins registry/schema behavior, search `tools.test.ts:428-439` pins descriptions,
and `packages/pi-plugin/src/index.test.ts:59-78` pins the Pi gate; all unaudited.
Impact: Internal source availability can be misreported as an agent feature,
or unsupported Pi scenarios can make cross-harness coverage appear complete.
Open questions:
- The Pi gate changed after the inspected revision: Pi now folds history, so
  "Pi P1-P5 transformation unsupported" no longer describes the shipped
  capability. Should the corpus gain Pi scenarios, or should reports state
  "Pi folding exists and is outside this corpus"? (needs human input)

## Relationships and handoff

The reopen property requires the binding property, but neither dominates the
other: correct identity with damaged bytes and intact bytes from the wrong
event are different failures. Neither establishes consumer reachability.
The unavailable-credit property owns the single three-way disposition and
zero-credit predicate. Delivery owns pressure construction and consumes that
predicate. The capability-report property validates the registry evidence
used by the memory witness; memory decision recovery never implies native
exactness.

The memory-recovery record shares the source, admission, and invocation
fixture with delivery's
[memory admission record](../delivery/catalog.md#cf-memory-credit-requires-admitted-visible-content).
Delivery checks automatic inclusion and its admission/pressure controls;
recovery adds visible-argument provenance and explicit tool-result delivery.
These are complementary assertions over one fixture, not duplicate fixtures.

The old daemon transcript properties remain normalization/persistence leads,
not duplicate native exactness records. Existing native retention and
descriptor tests remain their mechanism owners. Extend the highest existing
witnesses rather than creating a parallel retention or search implementation.

Route all five active records to `/testing:test-strategy` for test form and
oracle implementation. Route existing tests to `/testing:invariant-test-review`
and production guards to
`/low-level-systems:defensive-assertions-and-invariant-guards`. The
[fault map](fault-map.md) supplies situation markers and boundary constraints.
Semantic judgments require human review of independent obligations and actual
delivered text; these deterministic predicates cannot certify model behavior.

The independent portfolio review follows this discovery. No portfolio verdict
or `portfolio-evaluation.md` is supplied. No source, test, CI, tracker, model,
commit, or publication action is part of this documentation task.

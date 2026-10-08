# Compression fidelity: generation properties

## Scope and evidence boundary

Date: 2026-09-19. System: `crates/daemon` history summarizer generation, from
native input through accepted history publication, plus its proposed replay
evidence boundary. Inspected HEAD:
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.

The published
[settled contract](https://github.com/ahrav/eidnara/issues/707)
is the contract lead for R1/R2/R5/R7, KTD1/KTD2, and U5/KTD5. See
"Requirements", "Implementation Decisions", and "Milestone boundaries and
dependencies". The original plan's inspected SHA-256 is
`b33aaf8508174d39ea578cb6927d1524bbee1c3f2496c8a22511349beb35a384`.
It names revision `1555f00c702296406186f8c859045014e8e8a4d9`; HEAD has moved.
The intervening diff changes Curator activation, lifecycle, worker, tests, and
operations documentation. All code references here use the inspected HEAD,
not the plan's source offsets. Contract statements remain claims under test.
Plan line citations are historical provenance only; the
[catalog index](../README.md#scope-and-provenance) maps them to published
sections and records the original title/date and publication evidence.

External-evidence scope is already answered: the plan and local code, docs,
and history are supplied. No additional incident logs or related repositories
are supplied. C1-C6 are synthetic acceptance examples, not documented incidents.
No tests or CI checks run here. No external model calls or tracker mutations
occur. Existing checks are **unaudited**, regardless of their names.

The [older catalog](../../daemon/history_summarizer/catalog.md) is a lead, not
proof. An orchestrator-commissioned independent evaluation finds broken
record/index counts and obsolete evaluator CI claims there. This part neither
inherits those counts nor treats historical evaluator names as executable
checks at HEAD. The evidence-scope statement records what was supplied, not a
prohibition on consulting other evidence.

This part owns generation's cross-boundary consequences: what the producer
actually sees, what meaning a human requires, and which artifact supports an
evaluation claim. It does not duplicate atomic publication, generic healing,
tier content floors, serving/packing, admission, or exact-source recovery.
P5 omission and complete-invocation recovery belong to adjacent parts. This
part consumes their dispositions for permitted P4 omissions and reviews the
capability claims in producer/hint guidance without owning retrieval itself.
U5 correction evidence is in scope. No new API, persisted claim ontology, or
evaluator framework is proposed.

## System model and lens summary

The path is native messages and projected identities, chunk construction,
presented input and aliases, prompt assembly, producer output, validation and
healing, then publication. Human obligations and judgments sit outside that
path. Scripted output exercises mechanics; it is not model-quality evidence.

| System-model lens | Inspected result |
| --- | --- |
| Architecture and data flow | [Chunk assembly:974-1053][chunk] feeds the real presented input, calibration examples, prior summaries, and admitted memory to the prompt. |
| State and persistence | [Publication orchestration:2432-2564][driver] separates output, validation, and publication. [Stored conversion:48-83][driver] copies effective tiers, not their authored-presence history. |
| Concurrency | [Selected-range drift and tail-extension checks:4208-4306][driver] distinguish a changed source from later unrelated input. Fidelity evidence must retain that attribution. |
| Claimed safety | [Prompt:3-17][system-prompt] asks for decisions and constraints. R1/R2 add explicit temporal, polarity, scope, and evidence distinctions; schema acceptance cannot establish them. |
| Claimed liveness | No bounded semantic-convergence mechanism is found. Producer completion and retries are not semantic progress evidence. |
| Bug history and density | The inspected `a03f58d2` patch fixes trimmed text being called verbatim. [HEAD:271-274][chunk] uses exact native equality. This supports an identity hazard, not an observed semantic incident. |
| Existing test strategy | Prompt goldens, native alias checks, parser fixtures, and scripted publication exist. U1 (#718) adds the fidelity corpus and its validation module; the replay module and evaluation command are absent. |
| Failure and degradation | [Parser:320-341][validator] fills P2/P3 and defaults P4 empty. [Validation:645-709][validator] can discard coverage and reject facts while retaining history. |
| Dependencies | [Config:139-145][config] defaults to an empty model chain; [handler:5685-5692][lib] reports `no_models`. Real generation is configuration-dependent. |
| Product context | Losing rejection, a scoped limit, final status, or uncertainty changes the permitted next action in C1-C5. C6 also exposes the difference between native and presented evidence. |
| Unproven assumptions | [Tool summaries:1242-1259][chunk] omit result payloads. Covered ordinals and surviving aliases do not establish exposure of a decisive receipt. |
| Wildcard, last | [MemoryReviewer fixture `judge`:55-70][curator-corpus] is substring-based. Its source/script separation is reusable; its lexical classifier is not a general semantic oracle. |

Property passes follow the system model. Each row records a separate attention
focus, not independent corroboration by another reviewer.

| Property lens | Retained consequence |
| --- | --- |
| Data integrity | Bind obligations to native occurrence/revision and results to exact artifacts. |
| Concurrency | Keep stale-attempt rejection distinct from accepted generation coverage. |
| Failure recovery | Preserve failed attempts and fallback origin rather than reporting only the final result. |
| Protocol contracts | Separate structural acceptance from human semantic acceptance. |
| Resource boundaries | Account for normalization, tool summarization, and input truncation before judging generation. |
| Security boundaries | Keep case answers, forbidden conclusions, and review labels outside provider input. |
| Distributed coordination | No separate distributed protocol is owned here; endpoint fences and transport stay with their catalogs. |
| Lifecycle transitions | Track authored, parsed, healed, discarded, and published artifacts separately. |
| Idempotency and replay | Use one corpus identity across attempts; retain failed and superseded attempts. |
| Version compatibility | Keep KTD1's Rust-owned corpus and KTD2's private test ancestry without changing wire contracts. |
| Wildcard, last | Require a material source-to-publication situation, not a placeholder-prompt success or evidence found only in references. |

Focused portfolio refinements retain two further consequences. Prompt lines
130/212 promised recovery beyond the registered memory-only search contract,
and so did the hint footer until #926 rewrote it as a memory search, despite prompt lines 17/362 disclaiming transcript recovery.
The owning guidance needs manual capability review, not automated semantic
classification. U5 also needs a named failing witness, fresh reviewed same-case
outputs for meaning changes, and explicit importance/serving-shift observations
for rubric edits. Five additional situation markers distinguish transformed
input, actual-budget truncation, consumed fallback, discarded final coverage,
and inherited tiers; they do not broaden the single joined-path record.

The non-durable method-note path is
`/tmp/opencode/compression-fidelity-generation-lenses.md`. This summary retains
its durable findings; reading the catalog does not require that file. Fresh
portfolio evaluation remains the reviewer's work;
this author does not create `portfolio-evaluation.md`.

## Reachability and index

Reachability describes the property surface, not evidence that a test ran.
Production generation records are `explicit-config-only`: a model chain and
eligible firing are required. Corpus/review/campaign records, including manual
guidance review and U5 correction acceptance, are `test-only`: their checks
belong outside production, and their fidelity harness does not exist at HEAD.
The guidance text itself is production input, not a production semantic guard.
Each evidence file supplies the record-specific justification.

| Slug | Type | Reachability | Semantics | Contract |
| --- | --- | --- | --- | --- |
| [cf-source-obligation-independence](#cf-source-obligation-independence) | safety | test-only | always | R5/R7, KTD1 |
| [cf-producer-input-exposure](#cf-producer-input-exposure) | safety | explicit-config-only | always | R1/R5/R7, KTD2 |
| [cf-material-qualifier-fidelity](#cf-material-qualifier-fidelity) | safety | explicit-config-only | always | R1/R2/R5 |
| [cf-semantic-evidence-separation](#cf-semantic-evidence-separation) | safety | test-only | always | R7, KTD1/KTD2 |
| [cf-generation-stage-provenance](#cf-generation-stage-provenance) | safety | explicit-config-only | always | R2/R7, KTD2 |
| [cf-material-generation-reachability](#cf-material-generation-reachability) | reachability | test-only | sometimes | R5/R7, KTD2 |
| [cf-guidance-capability-bound](#cf-guidance-capability-bound) | safety | test-only | always | U5, KTD5 |
| [cf-witnessed-corrections](#cf-witnessed-corrections) | safety | test-only | always | U5, KTD5 |

### cf-source-obligation-independence

Type: safety
Reachability: test-only
Status: active
Exercised: partial - `crates/daemon/src/compression_fidelity_tests.rs`
validates the committed C1-C6 corpus's IDs, native spans, revisions,
required scenarios, and answer-key exclusion, with negative controls.
Human approval of the corpus and replay through real prompt assembly are
absent.
Guarantee: Generation evaluation uses source-backed human obligations fixed
independently of candidate wording and bound to one corpus identity.
Check: `always` - For each admitted replay, consume the successful identity
observation owned by [cf-corpus-byte-identity][corpus-owner] for that replay;
do not implement another hash validator. Require unique case IDs and
case/scenario pairs, valid native occurrence/revision/span references for
every obligation, and the declared C1-C6 cases. Require recorded human approval
of material obligations, forbidden conclusions, and allowed losses before
candidate inspection. Confirm that only permitted source/prompt sections enter
provider input, not answer keys or review labels. This is an admission
invariant for every replay, not an entailment check.
Fault/timing angle: Corpus loading, prompt construction, and baseline changes
can replace the independent oracle with candidate-derived expectations.
Required faults and enabling state: Consume accepted and rejected/missing
identity-owner observations; construct a wrong native revision, duplicate IDs,
candidate-derived obligations, and answer-key contamination against a valid
control. Source annotations must exist before output generation.
Confidence: medium - [Evidence](evidence/cf-source-obligation-independence.md).
The plan establishes the obligation; inspected fixture and private-test
patterns support placement, but no fidelity loader or review gate exists.
Existing check: `compression_fidelity_tests.rs` validates the shared corpus
(U1, #718), unaudited;
[native alias oracle:88-145][citations-golden] checks separate fixture
identities and bytes, unaudited.
Impact: A candidate can appear correct because its answer changed the expected
meaning or because different cases were compared.
Open questions:
- Who supplies the two initial corpus approvals required by the plan?
  (needs human input)
- Which native revisions and spans will the six reviewed cases freeze?
  (needs human input)

### cf-producer-input-exposure

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: not yet - No case-linked native-to-producer exposure observations
are captured.
Guarantee: Every material native obligation has an explicit exposure outcome
at the real producer input, and absent evidence receives no generation-fidelity
credit.
Check: `always` - For every case-marked obligation, require its native
identity/span and a linked observation of the exact assembled system text,
user prompt, and selected model. The private test recorder must capture all
three; its current user-prompt-only log is insufficient.
An exposed claim must identify the supporting presented fragment with its
transformation status, while omitted, filtered, or truncated evidence must be
recorded as unexercised input for that obligation. Machine checks verify
identities, fragments, and linkage; humans decide whether transformed text
still exposes the required meaning. This accounting applies to every attempted
generation.
Fault/timing angle: Normalization, role filtering, tool-result summarization,
and budget truncation occur before the producer sees the source.
Required faults and enabling state: Supply a decisive tool receipt, a scoped
one-off constraint, whitespace/Unicode changes, and an oversized first block
under the same build/present budget; include controls where material text
survives.
Confidence: high - [Evidence](evidence/cf-producer-input-exposure.md). The input
transformations and alias withdrawal are visible in inspected function bodies;
this is mechanism confidence, not a fidelity result.
Existing check: [Alias/truncation checks:347-522][citations-golden] and
[prompt golden:481-582][prompt], unaudited; none found for material-obligation
exposure accounting.
Impact: A loss before generation is misreported as a model error, or a model
receives credit for evidence it never saw.
Open questions:
- Which transformed fragments retain sufficient evidence for each obligation?
  (needs human input)
- How will the required private recorder extension retain system, user prompt,
  and model without exposing the answer key or exporting production internals?

### cf-material-qualifier-fidelity

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: not yet - No candidate tiers have been captured and reviewed against
the six cases' source obligations.
Guarantee: Each evaluated P1-P4 capsule preserves required subject, scope,
polarity, temporal state, and evidence status or records a case-permitted P4
omission disposition, without counting unavailable evidence as preservation
or strengthening the source claim.
Check: `always` - For every obligation required by the case at that tier,
require a human judgment on the title plus effective body that the required
meaning and qualifiers survive, no forbidden conclusion is supported, and
every loss is predeclared as allowed. For a case-permitted P4 omission, consume
the serving owner's complete-invocation disposition instead of demanding all
meaning in the P4 body. Visible or discoverable credit needs its observed
evidence; unavailable remains a preservation loss even if safe abstention is
permitted. A lower tier must not strengthen or contradict the source even if
its title is affirmative. Deterministic checks require the complete
artifact-bound judgment/disposition set but cannot compute semantic fidelity.
Every evaluated capsule is subject to this safety condition.
Fault/timing angle: Qualifiers can disappear during generation of a shorter
tier, or an early plan/repeated summary can displace the final primary evidence.
Required faults and enabling state: C1-C5 supply rejection/acceptance,
absent/present completion receipts, scoped limits, repeated misleading
summaries, supersession, partial/cancelled outcomes, and observed error versus
inferred cause; include acceptable paraphrases and the successful-deployment
control.
Confidence: medium - [Evidence](evidence/cf-material-qualifier-fidelity.md).
Prompt obligations and parser behavior are inspected; no actual model-fidelity
failure or success is established.
Existing check: None found for human-reviewed generation meaning;
[P1-only fallback fixture:1656-1677][validator] checks structure only, unaudited.
Impact: Later work revives a rejected option, violates a scoped constraint, or
treats an unconfirmed action or inferred cause as fact.
Open questions:
- What are the approved material obligations and tier-specific allowed losses
  for each case? (needs human input)
- Which P4 titles satisfy the safe-capsule requirement without relying on
  unproven recovery? (needs human input)

### cf-semantic-evidence-separation

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - No generation-fidelity review importer or accepted
semantic baseline exists.
Guarantee: Structural success, scripted output, and citation validity never
substitute for reviewed semantic evidence about a real producer output.
Check: `always` - Every accepted semantic result must reference the same
corpus and exact captured input/output artifacts as its recorded human
judgment, identify whether output was scripted or produced by the named real
model, and satisfy the plan's human review rule with no missing or disputed
required judgment. Scripted, structurally accepted, unreviewed, or failed
attempts alone cannot receive a real-model semantic-pass label. The automatic
check validates evidence classification and linkage, not the truth of the
human judgment, for every result.
Fault/timing angle: A report can promote parser success or a scripted happy
path into a semantic conclusion after generation completes.
Required faults and enabling state: Compare structurally valid good and
qualifier-reversing outputs with valid source spans; supply missing judgment,
wrong artifact linkage, and scripted-as-real controls without making a network
call.
Confidence: high - [Evidence](evidence/cf-semantic-evidence-separation.md).
Inspected validation and citation checks have no meaning oracle; the proposed
semantic result gate is absent and unproven.
Existing check: [Citation checker:141-172][citations] and
[fact-set fixture:188-281][citations-golden], unaudited; none found for semantic
acceptance or review completeness.
Impact: A green parser test or valid citation is presented as evidence that
compression preserves meaning.
Open questions:
- Who reviews the initial real-output baseline and adjudicates disputed
  judgments? (needs human input)
- Which provider/model and bounded capture configuration will a later
  authorized run use? (needs human input)

### cf-generation-stage-provenance

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: not yet - Existing stage tests have not been run, and no fidelity
replay joins their observations.
Guarantee: Generation evidence distinguishes authored output, parser fallback,
healed coverage, extraction outcome, and actual publication for the same
source-bound attempt.
Check: `always` - For each attempt, retain the raw output and link its authored
tier presence and ranges to parsed effective tiers, validated ranges/discard
outcome, extraction outcome, and observed published rows or explicit
nonpublication. Any claimed published tier/body and coverage must match those
rows for that attempt, and inherited P2/P3 or empty-default P4 must not be
labeled independently authored. Retries retain separate attempt records.
These equalities and classifications hold at every stage, without treating
covered ordinals as semantic preservation.
Fault/timing angle: Parsing erases authored omissions, healing changes ranges,
discard removes a final segment, and a later fallback can hide the failed
primary attempt.
Required faults and enabling state: Use P1-only XML, explicit empty P4, lenient
tier closure, tool-only healing, final-segment discard, citation rejection with
valid history, length-capped output, and a rejected primary followed by valid
fallback; correlate the selected native identities.
Confidence: high - [Evidence](evidence/cf-generation-stage-provenance.md). The
stage transformations and separate store result are inspected; no joined
provenance witness is exercised.
Existing check: [Parser fallback:1656-1730][validator],
[publication happy path:4154-4204][driver], and
[validation fallback:6389-6420][driver], unaudited; none found for the complete
fidelity provenance join.
Impact: A skipped or inherited tier looks authored, discarded material looks
covered, or a failure is hidden behind another attempt's publication.
Open questions:
- Which existing private observations suffice to retain pre-fallback authored
  presence and pre-heal ranges without widening production APIs?
- Does the joined witness remain correctly attributed across the changed
  Curator lifecycle at the inspected HEAD?

### cf-material-generation-reachability

Type: reachability
Reachability: test-only
Status: active
Exercised: not yet - No fidelity campaign constructs a case-linked material
source-to-publication witness.
Guarantee: A generation campaign reaches at least one real
assembly-to-publication situation containing independently annotated material
source evidence.
Check: `sometimes` - The constant marker
`cf-material-generation-reachability` fires when one attempt has a preannotated
material native source, its decisive fragment observed in the real assembled
`new_messages`, a consumed scripted or captured producer output, and nonempty
published history correlated to that same source and attempt. The marker
requires neither semantic failure nor success and fires on a correct
implementation. It checks a meaningful situation, not mere function entry.
Fault/timing angle: A placeholder prompt, no-model configuration, no-fire
boundary, validation rejection, or unobserved publication can make the safety
records vacuous.
Required faults and enabling state: Use the private test driver with an
explicit model chain, eligible native messages and identities, a firing
boundary, sufficient input budget, valid source-bound XML, and a store read
observing the publication; no external model is required.
Confidence: medium - [Evidence](evidence/cf-material-generation-reachability.md).
Private prompt capture and publication seams exist, but their case-linked
combination is not constructed here.
Existing check: [Scripted publication test:4154-4204][driver] uses a placeholder
prompt, unaudited; none found for this material situation marker.
Impact: Generation properties appear green even though no material obligation
traversed the path being evaluated.
Open questions:
- Can the private orchestration seam construct the first complete witness with
  existing helpers alone?
- After that witness, do all required C1-C6 generation scenarios run? One fired
  marker does not establish complete case coverage.

### cf-guidance-capability-bound

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - Static guidance/capability contradictions are inspected,
but no artifact-bound guidance review or corrective replay is executed.
Guarantee: Producer and hint guidance makes no recovery claim beyond the tools
and sources available in the registry configuration to which it applies.
Check: `always` - For each reviewed guidance revision, require a human review
of its recovery claims against the linked registry, tool schema, and execution
path for the applicable configuration. Each claim must have a supported route
or be narrowed to state the limitation; missing registration, memory-only
search, or stored history alone cannot support a history/transcript recovery
promise. Deterministic checks bind that judgment to exact guidance and
capability artifacts, not classify the prose automatically. The condition
applies to every guidance revision accepted as capability-correct.
Fault/timing angle: Prompt or hint wording can preserve an obsolete recovery
premise after tools or source contracts change, or contradict another section.
Required faults and enabling state: Review prompt lines 130/212 against 17/362
and a history-derived hint with the memory-only registry; include a supported
memory-search statement and a disabled-registry control. Link the actual text
and applicable configuration without requiring an external provider call.
Confidence: high - [Evidence](evidence/cf-guidance-capability-bound.md).
The prompt conflict, history-derived hint footer, and registered memory-only
search path are inspected; this is evidence of a false premise, not a passing
guidance review or an observed consumer incident.
Existing check: [Registry guidance test:272-319][registry-tests] and
[search-description test:428-439][search-tests], unaudited. They do not cover
the summarizer prompt and hint footer together; no complete review check is found.
Impact: Generation discards necessary qualifiers or detail on a recovery
assumption the consumer cannot fulfill.
Open questions:
- Which narrowed producer/hint wording will a human approve against the linked
  registry capabilities? (needs human input)
- How will the offline review retain the applicable configuration and exact
  generated hint alongside its capability evidence?

### cf-witnessed-corrections

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - No U5 correction, discriminating replay, or fresh reviewed
same-case semantic comparison is produced here.
Guarantee: A local fidelity correction is accepted only with a prior named
failing witness, verified improvement without required-obligation regressions,
and the fresh semantic and serving evidence required by its change type.
Check: `always` - For each proposed U5 correction, require a named source-bound
failing witness captured before the fix and a matching post-fix observation
that resolves it. Consume [complete comparison evidence][comparison-owner]
for the same cases and scenarios, with no required-obligation regression or
hidden failure. Unresolved required baseline failures remain reported and block
overall fidelity acceptance. Prompt or generated-meaning changes require fresh
real outputs and human review, not only a golden update. Rubric edits also
require before/after importance outputs and effective serving choices, with
all shifts disclosed rather than assumed unchanged. Check the recorded U2/U3
prerequisites for deterministic text corrections and U4 evidence for meaning
changes. These are correction-acceptance conditions, not an automatic meaning
classifier or an authorization to change production policy.
Fault/timing angle: A speculative prompt cleanup, self-updated oracle, reused
old review, or hidden importance shift can make an unwitnessed fix look proven.
Required faults and enabling state: Supply a named failure and matched fix,
then controls with no prior witness, a golden-only prompt change, a required
obligation regression, and a rubric edit missing effective-serving evidence.
Preserve the original cases and declared constraints throughout comparison.
Confidence: high - [Evidence](evidence/cf-witnessed-corrections.md). U5's
acceptance rule is explicit; inspected importance storage and tier selection
show why rubric changes need serving observations. No correction is verified.
Existing check: [Prompt golden:481-582][prompt] and
[importance storage check:3186-3219][driver], unaudited; none found for the
prior-witness, fresh semantic review, and serving-shift acceptance gate.
Impact: A text change earns a fidelity claim without evidence, or improves one
case while losing a required obligation or changing what later tiers expose.
Open questions:
- Which named failing witness and localized correction will proceed after the
  plan's prerequisites are met? (needs human input)
- Who reviews the fresh same-case outputs and the significance of any observed
  importance/serving shifts? (needs human input)

## Relationships and handoff

`cf-source-obligation-independence` consumes
[cf-corpus-byte-identity][corpus-owner], whose evaluation owner implements the
single hash validator, and supplies the independent obligations used by other
records. `cf-producer-input-exposure` separates input loss from
generation loss. `cf-generation-stage-provenance` identifies the capsule that
`cf-material-qualifier-fidelity` asks humans to judge.
`cf-semantic-evidence-separation` prevents mechanical success from replacing
that judgment. `cf-material-generation-reachability` counters vacuity in the
joined path, but does not dominate per-case completeness or human review.
No implication between identity correctness and semantic correctness is claimed.
`cf-guidance-capability-bound` owns the manual review of promises, not tool
recovery correctness. `cf-witnessed-corrections` consumes those findings and
[complete comparison evidence][comparison-owner] for U5. It does not create a
second evaluator. The [fault-map markers](fault-map.md#coverage-checks-to-add)
and the evaluation owner's completeness check cover situations and case rows
that the narrow joined-path reachability record does not establish.

Reuse endpoint checks in the older catalog rather than restating their
invariants. A correct atomic publish can still publish misleading prose; a
correct healing rule can still enlarge a range without restoring unseen input.
Those consequences, not new endpoint rules, are the subject of this part.

For each active record, `/testing:test-strategy` selects the cheapest valid
boundary and oracle. Corpus identity, input exposure, provenance, and situation
coverage can use local deterministic observations. Semantic content requires
human review of captured real outputs. `/testing:invariant-test-review` owns
adequacy of the listed tests; production guard strength belongs to
`/low-level-systems:defensive-assertions-and-invariant-guards`. No distributed
simulation requirement is inferred from these records.

KTD1 keeps one Rust-owned corpus with byte identity owned by the evaluation
part; KTD2 places the replay
under [the existing private test module:18252-18274][lib]. The private
[ProducerState:22322-22360][lib] and [handler helper:22626-22652][lib] exist.
U1 (#718) adds the corpus; the replay module and evaluation command do not
exist yet. Their absence is
a handoff dependency, not permission to export internals or create a framework.
The test recorder must be extended to retain the complete system text, user
prompt, and selected model required by generation claims. Its existing
[start method:22416-22441][lib] ignores system/model arguments. Keep that
extension under private test ancestry; no production export is needed.

Preserve the settled unit order: U1 defines the corpus; U2 depends on U1; U3
depends on U1/U2 and qualifies its own fixture lane; U4 depends on U2 and the
qualified U3 seam. U5 uses U2/U3 for deterministic text corrections and U4 for
meaning changes. A cheap qualification probe does not replace U1 or authorize
corrections before those prerequisites. No extra blinding gate is introduced.

Portfolio totals: eight active records, eight index rows, eight evidence files;
seven safety and one reachability; seven `always` and one `sometimes`; three
`explicit-config-only` and five `test-only`. No liveness record is added
because semantic preservation is not a bounded recovery claim. Seven remain
`Exercised: not yet`; cf-source-obligation-independence is partial after U1. See [existing checks](existing-checks.md) and
[fault mapping](fault-map.md) for the unaudited evidence and missing harnesses.

Mechanical documentation validation passes for field order, record/index/file
agreement, evidence headings and lengths, links, and source line bounds.
Referenced tracked source files match the inspected HEAD; the local plan digest
is unchanged. These checks do not exercise any cataloged system property.

[chunk]: ../../../../crates/daemon/src/history_summarizer_chunk.rs
[prompt]: ../../../../crates/daemon/src/history_summarizer_prompt.rs
[system-prompt]: ../../../../crates/daemon/testdata/history_summarizer-system-prompt.txt
[validator]: ../../../../crates/daemon/src/history_summarizer_validate.rs
[citations]: ../../../../crates/daemon/src/history_summarizer_citations.rs
[citations-golden]: ../../../../crates/daemon/src/history_summarizer_citations_golden.rs
[driver]: ../../../../crates/daemon/src/history_summarizer.rs
[config]: ../../../../crates/daemon/src/config.rs
[lib]: ../../../../crates/daemon/src/lib.rs
[curator-corpus]: ../../../../crates/daemon/tests/support/memory_reviewer_corpus.rs
[corpus-owner]: ../evaluation/catalog.md#cf-corpus-byte-identity
[comparison-owner]: ../evaluation/catalog.md#cf-complete-independent-comparison
[registry-tests]: ../../../../packages/opencode-plugin/src/plugin/tool-registry.test.ts
[search-tests]: ../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts

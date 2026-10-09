# Generation: fault and situation map

Date: 2026-09-19. System: `crates/daemon` history summarizer generation.
Inspected HEAD: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
The [catalog](catalog.md#scope-and-evidence-boundary) records the supplied plan,
its digest, revision drift, and external scope. No additional incident logs or
related repositories are supplied. No faults are injected and no tests run here.

Fault availability means an inspected seam can represent a condition, not that
this discovery exercised it. All listed existing checks are unaudited.
Synthetic semantic mutations below are acceptance controls, not incidents.

## Fault classes and availability

| Condition | Available mechanism or missing dependency | Owning records |
| --- | --- | --- |
| Wrong source occurrence/revision/span; equal-length change | Native alias fixtures and selected-identity drift checks exist. U1 (#718) validates corpus spans and revisions; replay through real assembly is absent. | cf-source-obligation-independence; cf-producer-input-exposure; cf-generation-stage-provenance |
| Corpus digest mismatch | The evaluation part's [cf-corpus-byte-identity][corpus-owner] owns validation. Generation consumes its accepted/rejected observation; no second hash validator is proposed. | Evaluation owner; cf-source-obligation-independence consumes its result |
| Duplicate case IDs or candidate-derived answer key | None found for the planned source-obligation checks. | cf-source-obligation-independence |
| Answer key enters generation input | Real prompt assembly and private prompt capture exist; fidelity source/answer separation check is absent. | cf-source-obligation-independence |
| Decisive text normalized, filtered, or replaced by a tool-call summary | Existing native message constructors can supply these inputs. Tool-result payload omission is visible in `extract_tool_result_summaries`. | cf-producer-input-exposure |
| Decisive suffix truncated | Same-budget oversized-first-block fixture exists. Do not rely only on a smaller second-stage budget that production does not use. | cf-producer-input-exposure |
| Rejection reversed; planned becomes completed; inferred becomes observed | Scripted XML can carry the mutation while preserving valid structure. Human semantic judgments and real-output captures are absent. | cf-material-qualifier-fidelity; cf-semantic-evidence-separation |
| Scope/value/unit lost; repeated summary overrides primary evidence | C3-C5 specify the source situations. No generation-fidelity corpus fixture exists yet. | cf-material-qualifier-fidelity |
| Missing P2/P3, empty P4, lenient closure | Parser fixtures exist. Effective tiers alone cannot recover authored presence. | cf-generation-stage-provenance; cf-material-qualifier-fidelity |
| Tool-only healing or provisional final-segment discard | Validation functions and checks exist. The joined fidelity trace is absent. | cf-generation-stage-provenance |
| Valid history with rejected facts | Fact-set/citation fixtures exist. Validation success must not be called extraction success or semantic success. | cf-generation-stage-provenance; cf-semantic-evidence-separation |
| Length-capped output or invalid primary followed by valid fallback | Scripted producer cases exist. A fidelity observation must retain each attempt, not only the successful model. | cf-generation-stage-provenance |
| Missing human judgment, wrong artifact linkage, scripted-as-real label | None found for the planned semantic result gate. These are offline metadata controls, not external calls. | cf-semantic-evidence-separation |
| No models, no eligible boundary, placeholder prompt, no publication | Existing gates and private test helpers exist. No material source-to-publication situation marker exists. | cf-material-generation-reachability |
| Unsupported prompt/hint recovery promise or changed tool registration | Prompt lines 130/212 conflicted with the memory-only search route at HEAD `99f68bd3`; #723 rewrote them and #926 aligned the hint footer. Registry/schema tests and the prompt guidance-capability test exist; joint manual guidance review and artifact linkage are absent. | cf-guidance-capability-bound |
| Fix without prior failure, golden-only prompt change, stale review, or hidden rubric effect | U5 states the corrective evidence rule. No fidelity correction gate or fresh same-case comparison exists. | cf-witnessed-corrections |

Detailed file/function evidence and failure scenarios live in each record's
evidence file, linked from the [catalog](catalog.md#reachability-and-index).

## Required setup per property

| Property | Required enabling state and observation | What prevents a vacuous result |
| --- | --- | --- |
| cf-source-obligation-independence | One reviewed C1-C6 corpus, the evaluation owner's identity result, native source references, and captured provider input. | Reject missing/failed owner evidence and exercise source-revision/leak controls. Do not duplicate the digest comparison. A loader that refuses every case is not a successful campaign. |
| cf-producer-input-exposure | A material annotation predates output; native messages traverse real build and prompt assembly under recorded budget. | Pair a lost or transformed decisive fragment with a surviving control. Report input gaps, not model-fidelity passes. |
| cf-material-qualifier-fidelity | Required tier obligations and allowed losses exist before reviewing title plus effective body; a permitted P4 omission consumes the serving owner's complete-invocation disposition. | Include acceptable paraphrases and C2 with a genuine success receipt. Unavailable never earns preservation/recovery credit, even if safe abstention is permitted. |
| cf-semantic-evidence-separation | Captured input/output, scripted/real origin, corpus identity, and artifact-bound human judgment are independently available. | A structurally accepted bad output and a structurally accepted good control must remain distinguishable by recorded human judgment. |
| cf-generation-stage-provenance | The same source-bound attempt has raw XML, parsed output, validated coverage, extraction status, and observed publication or explicit failure. | Include inherited tiers, changed/discarded coverage, and a fallback attempt. Nonempty storage alone cannot establish origin. |
| cf-material-generation-reachability | Real assembly exposes a decisive native fragment, the driver consumes output, and the store shows matching nonempty history. | The marker describes these preconditions and observations, not a semantic violation. A correct scripted output can satisfy it. |
| cf-guidance-capability-bound | Exact prompt/hint text, applicable registry configuration, schema/execution evidence, and a linked human guidance judgment. | Compare the unsupported history-recovery promise with a supported memory-search statement and a disabled-registry control. A string check alone cannot approve meaning. |
| cf-witnessed-corrections | A named pre-fix failure, matched post-fix evidence, required U2/U3 or U4 prerequisites, and same-case reviewed comparisons. | Reject missing prior witness, golden-only semantic claims, required-obligation regressions, and rubric changes without importance/effective-serving observations. |

All eight remain `Exercised: not yet`. A missing input fragment is a generation
prerequisite gap and may expose a pipeline preservation failure; it is not a
reason to erase the case or mark overall fidelity successful. Likewise,
publication refusal is an observed stage outcome, not a semantic pass.

## Coverage checks to add

The catalog's narrow joined-path record defines one constant situation marker:
`cf-material-generation-reachability`. It uses `sometimes` because the campaign
must produce a complete material situation, not merely enter a function.
It never requires an incorrect summary or negates a passing safety condition.

Add these five separate constant, globally unique markers. Each uses
`sometimes` because its named situation must occur in the campaign declaring
that scenario. All are setup/outcome preconditions that can occur with correct
accounting and faithful output, not assertions that a bug happened. None exists
in the fidelity harness at HEAD, and none is exercised here.

| Marker | Exact witnessed condition | Discriminating control and existing mechanism |
| --- | --- | --- |
| `cf-generation-material-transformed` | A preannotated material native fragment traverses real assembly, its linked presented fragment changes through an inspected transformation, and the captured alias marks it transformed. No semantic loss is required. | An unchanged single-block fragment must not satisfy it. [Whitespace transformation:509-522][golden] supplies the local mechanism. |
| `cf-generation-material-truncated` | Real build and presentation use the same actual configured budget; the truncation marker is observed and the case's decisive native fragment lies outside the kept presented prefix. Withdrawal of a whole-part alias alone is insufficient. | A sufficient-budget control retains that fragment and must not satisfy it. [Same-budget truncation:457-482][golden] supplies the local mechanism. |
| `cf-generation-primary-rejected-fallback-consumed` | One source-bound firing records the primary validation rejection, then a distinct fallback attempt whose output is actually consumed; a start counter alone is insufficient. | A rejected primary with no consumed fallback must not satisfy it. [Fallback test:6389-6420][driver] supplies the local sequence. |
| `cf-generation-final-discard-with-earlier-coverage` | Raw output contains at least two segments; validation retains nonempty valid earlier coverage and records the final segment as discarded. | A single segment or force-kept final segment must not satisfy it. [Discard control:311-343][golden] distinguishes earlier retained coverage from final discard. |
| `cf-generation-p2-p3-inherited` | Captured raw output authors P1 but omits P2/P3; parsed effective P2/P3 inherit the denser text, and the observation records that neither was authored. | Explicitly authored equal P2/P3 strings must not satisfy it. [P1-only fallback:1656-1677][validator] supplies the local mechanism. |

For all five markers, bind the observations to a declared case and real prompt
assembly. A `placeholder prompt` control or a `no_models` no-fire outcome must
fire none of them and must not fire `cf-material-generation-reachability`.
The existing endpoint tests identify mechanisms; their isolated invocations do
not satisfy these case-linked campaign markers without the required setup.

Separate case/scenario observations must record all six required cases and
their declared controls. Do not construct dynamic marker names from case IDs.
The joined-path marker remains a minimum witness, not a completeness check.
The evaluation owner's [complete comparison][comparison-owner] reconciles
required scenario and attempt rows; firing any or all markers is not a semantic
pass. For an unfired marker, distinguish missing setup from a path that has
become unreachable. A finite campaign does not establish or refute unbounded
semantic liveness.

No new liveness timer is needed here. Existing producer retry and publication
mechanics retain their own deadlines and tests. Their outcomes are inputs to
provenance accounting, not a new convergence protocol.

## Leverage within the settled dependency order

1. **U1:** Establish independent corpus obligations, native references, and
   answer-key exclusion. Consume byte-identity evidence from its evaluation
   owner rather than building another hash check.
2. **U2, after U1:** Reuse private test ancestry and scripted execution for
   input-loss, parser, fallback, and publication observations. Extend the test
   recorder to capture full system text, user prompt, and model. No production
   export is needed. These are the cheapest generation situation witnesses.
3. **U3, after U1/U2:** Qualify its existing fixture/provider lane and obtain
   the serving and registry observations consumed here. A qualification probe
   does not move U3 ahead of the corpus and replay dependencies.
4. **U4, after U2 and qualified U3:** Bind human judgments to exact artifacts
   and capture/review bounded real outputs when separately authorized. Offline
   good/bad samples qualify accounting, not model meaning. No such call runs
   in this lane.
5. **U5:** Use a prior named failing witness before a localized correction.
   Deterministic text corrections require U2/U3 evidence; meaning changes
   require U4 and fresh reviewed same-case outputs. Disclose rubric-driven
   importance and effective-serving shifts. A golden update alone is not proof.

These are routing recommendations for `/testing:test-strategy`, not a chosen
test framework. The fresh portfolio reviewer owns harness-fit, balance, and
implementability judgments. Generic healing, publication atomicity, serving,
and exact-source recovery stay with their existing or adjacent owners.

[corpus-owner]: ../evaluation/catalog.md#cf-corpus-byte-identity
[comparison-owner]: ../evaluation/catalog.md#cf-complete-independent-comparison
[golden]: ../../../../crates/daemon/src/history_summarizer_citations_golden.rs
[driver]: ../../../../crates/daemon/src/history_summarizer.rs
[validator]: ../../../../crates/daemon/src/history_summarizer_validate.rs

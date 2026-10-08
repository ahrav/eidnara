# Evaluation fault and situation map

Date: 2026-09-19. Repository: `ahrav/eidnara`.
Inspected HEAD: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
The [catalog](catalog.md) records the supplied plan, revision drift, scope, and
claim-versus-implementation boundary. Only that plan and local code/docs/history
were consulted. No extra incidents or related repositories were consulted.
No fault or test in this map was executed during discovery.

## Fault availability

“Available” means a local seam or ordinary input operation exists, not that it
has exercised these properties. “Missing” identifies planned work, not a request
to build another framework.

| Class | Fault or situation | Availability at HEAD |
| --- | --- | --- |
| E1 | Missing/unreadable corpus, stale compiled corpus, whitespace/source/scenario byte drift | File/read-failure inputs and SHA-256 libraries exist; the shared corpus and digest exchange are missing. `hermetic-host.ts:306-330` can select an existing prebuilt fixture path. |
| E2 | Unknown case, alias mismatch, queue exhaustion, wrong consumer | Control framing and backend success/block/failure exist at `direct_host_fixture.rs:604-634,749-802`; case scripts and consumption observations are missing. |
| E3 | Backend completion without accepted nonempty publication | Existing sentinel backend and counter test expose the distinction. Required ungated qualification is missing; broad fold execution remains gated. |
| E4 | Capture reset, absent artifact, cross-attempt binding, stale derived row | `server.ts:169-171` and `scripted-tool-call.ts:101` supply resets; deleting/tampering with local files is possible. The authoritative assembler is missing. |
| E5 | Interrupted JSON publication | `atomic-publish.ts:5-16` supplies temp-plus-rename. The incident test manually leaves a temporary file. Neither is a crash-durability harness. |
| E6 | Missing/duplicate attempt, retry/fallback selection, baseline drift | Incident report validators and fake-child tests supply partial outcome prior art. Fidelity attempt identities and baseline comparison are missing. |
| E7 | Unreviewed output or model/human disagreement | Human authority is a plan obligation. No review evidence or semantic evaluator exists. |
| E8 | Disabled forwarding, wrong HTTPS target, credential leakage | The local Messages mock exists and stores all incoming headers. Live forwarding, synthetic-data admission, and artifact exclusion checks are missing. |
| E9 | Wrong selected model, mismatched response, missing tool result | Scripted tool loops correlate `tool_use_id`; the harness hardcodes `mock-sonnet`. Real response forwarding and capture are missing. |
| E10 | Provider error/timeout, output/call/time/spend exhaustion | The mock can emit errors/delay and tool calls. Whole-run real-provider accounting and immutable evaluation limits are missing. |
| E11 | Constant verdicts, missing controls, altered expected labels | U4 scenario 2 supplies known-bad and positive-control obligations. Sealed-label/verdict comparison is missing. |
| E12 | Missing usage identity, omitted attempts/cost, pooled cold/warm results, hidden raw leakage | Mock usage and normalized size helpers exist, but complete fidelity cost observations do not. `DiagnosticSink` at `incident-pool/support/case-workspace.ts:129-159` bounds private diagnostic output, not provider spend. |

Source paths in this table are under `packages/e2e-tests/src/` except the Rust
fixture under `crates/daemon/examples/`. Full anchors appear in the
[existing-check inventory](existing-checks.md) and per-record evidence.

## Required faults and enabling state by property

| Property | Required situation and fault | Observable check boundary |
| --- | --- | --- |
| [cf-corpus-byte-identity](evidence/cf-corpus-byte-identity.md) | E1 with unchanged/differing bytes, missing file, and read failure. | Admission rejects missing/unreadable input and mismatched raw-byte hashes before replay/comparison. No canonicalization is allowed. |
| [cf-fixture-script-qualification](evidence/cf-fixture-script-qualification.md) | E2/E3 after U1/U2, with a valid alias-bound script plus wrong-ID/request and exhausted-queue inputs; include missing prerequisites. | Initial ungated U3 qualification observes actual request, consumption, accepted nonempty publication, and matching capture. Downstream delivery/recovery still need per-invocation evidence. |
| [cf-authoritative-evidence-assembly](evidence/cf-authoritative-evidence-assembly.md) | E4/E5 after at least one retained observation and before report derivation. | Hash retained artifact bytes and resolve owner identities. Reject missing/inconsistent evidence; never trust a conflicting derived table. |
| [cf-complete-independent-comparison](evidence/cf-complete-independent-comparison.md) | E6/E7 with identical case/scenario identity and mixed outcomes, retries/fallbacks, reviewed failures, and unresolved/unreviewed rows. | Reconcile every configured attempt and actual dispatch observation. Apply human authority and fidelity blockers independently of compression savings. |
| [cf-semantic-control-discrimination](evidence/cf-semantic-control-discrimination.md) | E11 with the complete independent known-bad and positive controls, then altered/missing/constant recorded verdicts. | Compare control identities and recorded verdicts to sealed labels before reviewable comparison. This is not semantic proof. |
| [cf-evaluation-cost-completeness](evidence/cf-evaluation-cost-completeness.md) | E10/E12 with cold/warm, generation, recovery, raw leakage, retry/fallback, and ambiguous attempts. | Reconcile all observed metrics/costs with attempt identity; unknown cost is not zero, and predeclared limits cannot change. |
| [cf-reviewed-semantic-batch-reached](evidence/cf-reviewed-semantic-batch-reached.md) | Required U2/U3 scenarios, authorized real outputs, actual complete tool loops where required, and human reviewers. E3/E7/E9 can prevent reachability. | Witness the complete reviewed real-output batch, even if its semantic verdict is failing. Offline or gated rows cannot substitute. |
| [cf-bounded-record-and-forward](evidence/cf-bounded-record-and-forward.md) | E8/E9/E10 with explicit configuration, reviewed synthetic input, canary credentials, repeated tool requests, and frozen limits. | Require exclusive recorded script/forward modes, actual response/tool correlation, and no credential capture/config/log persistence. Forwarding consumes the cost record's whole-loop limits. |

Required safety checks hold while the adverse inputs are active. No eventual
successful recovery is promised. The plan declares call count, maximum output,
timeout, and spend before a real comparison; these bounds cannot be increased
after failure. A missing provider response does not prove zero work or zero cost.
Track attempted sends and acknowledged responses separately per invocation.
Do not assert equal remote effects and attempts: the provider contract does not
establish one effect per request or observable exactly-once billing.

## Named safety checks and coverage checks to add

These required safety assertions are separate from occurrence markers. In
particular, optional enabled-mode safety cannot make the default-mode assertion
optional. Names describe checks to implement, not existing instrumentation.

| Constant assertion | Semantics | Required condition |
| --- | --- | --- |
| `cf-eval-offline-zero-outbound` | always | Every scripted/default execution dispatches zero outbound provider calls, including when real credentials are present in the test environment. |
| `cf-eval-mode-origin-consistent` | always | A real-model label requires recorded real execution and matching dispatch/response evidence, not a request/response model string; script and forward modes never silently mix. |
| `cf-eval-control-verdicts-match` | always | A reviewable comparison has exactly the sealed control IDs and matching recorded verdicts, with unchanged independent labels. |
| `cf-eval-cost-observations-complete` | always | Cost-complete status requires the cost record's per-attempt observations, phase separation, and unchanged predeclared limits; ambiguous costs are not zeroed. |

These are proposed constant marker names, not implemented instrumentation.
Each marker describes preconditions that can occur on a correct implementation.
None asserts that corrupted evidence was accepted or that a limit was exceeded.
Scenario IDs belong in observations, not dynamically generated marker names.

| Constant marker | Semantics | Independent situation |
| --- | --- | --- |
| `cf-eval-corpus-bytes-differ` | sometimes | Compiled and runtime inputs intentionally differ while both are available for admission checking. |
| `cf-eval-corpus-missing` | sometimes | Admission receives a nonexistent corpus path. |
| `cf-eval-corpus-unreadable` | sometimes | Admission encounters a corpus read failure. |
| `cf-eval-script-valid-and-exhausted` | sometimes | A valid matching script was consumed and another request arrives with the queue empty. |
| `cf-eval-script-alias-mismatch` | sometimes | A producer request and selected case script have intentionally nonmatching aliases. |
| `cf-eval-publication-observed` | sometimes | Initial ungated U3 qualification observes nonempty publication and a correlated nonempty provider request after U1/U2. |
| `cf-eval-capture-reset-boundary` | sometimes | An observation is retained before a helper clears in-memory capture history. |
| `cf-eval-derived-capture-conflict` | sometimes | A supplied derived row intentionally contradicts an intact owner capture. |
| `cf-eval-incomplete-attempt-set` | sometimes | A declared multi-attempt run includes failed/unreviewed attempts and retry or fallback observations. |
| `cf-eval-control-reversed-negation` | sometimes | The sealed control set presents a reversed-negation sample for judgment. |
| `cf-eval-control-false-completion` | sometimes | A planned-to-completed control is presented. |
| `cf-eval-control-false-observation` | sometimes | An inferred-to-observed control is presented. |
| `cf-eval-control-wrong-source` | sometimes | A wrong-source-identity control is presented. |
| `cf-eval-control-hint-qualifier-loss` | sometimes | A hint with a lost qualifier is presented. |
| `cf-eval-control-acceptable-paraphrase` | sometimes | A sealed acceptable-paraphrase control is presented. |
| `cf-eval-control-confirmed-deployment` | sometimes | A successful-deployment control with its receipt is presented. |
| `cf-eval-cost-cold-rematerialization` | sometimes | Cold rematerialization produces cost observations. |
| `cf-eval-cost-warm-repeat` | sometimes | A warm repeat produces separately attributed cost observations. |
| `cf-eval-cost-raw-leakage-present` | sometimes | Repeated raw source is present in the captured serving input for accounting. |
| `cf-eval-cost-recovery-observed` | sometimes | A recovery call and its output produce cost observations. |
| `cf-eval-cost-ambiguous-attempt` | sometimes | A sent attempt lacks a conclusive response/usage receipt. |
| `cf-eval-reviewed-real-batch` | sometimes | The complete real-output batch and required human reviews are present. |
| `cf-eval-offline-default-invoked` | sometimes | The capture boundary is invoked without forwarding; the separate zero-outbound assertion must run. |
| `cf-eval-forward-tool-result-observed` | sometimes | A provider tool call receives its registered-tool result in the next captured request. |
| `cf-eval-forward-call-limit-boundary` | sometimes | An enabled loop reaches the frozen call-count admission boundary. |
| `cf-eval-forward-output-limit-boundary` | sometimes | An enabled loop reaches the frozen output boundary. |
| `cf-eval-forward-time-limit-boundary` | sometimes | An enabled loop reaches the frozen deadline. |
| `cf-eval-forward-spend-limit-boundary` | sometimes | An enabled loop reaches the frozen spend-admission boundary. |
| `cf-eval-forward-credential-canary` | sometimes | Outbound authentication uses a canary before capture/config/log artifacts are inspected. |

The optional live-path safety record uses `always-or-unreached`, allowing an
offline deterministic campaign not to enable forwarding. The default invocation
marker and `cf-eval-offline-zero-outbound` are both required; they do not satisfy the
required real-batch marker. For full acceptance, an unfired real-batch marker
means missing workload/access/review or unreachable required behavior, never
optional success. Do not run external calls to clear it without authorization.

## Cheapest valid oracle and routing order

1. **Byte identity.** Plain raw-byte digest comparison catches stale builds and
   corpus edits without a provider. Route `cf-corpus-byte-identity` first.
2. **Artifact authority and completeness.** Small intact/tampered/missing
   artifact sets and outcome tables can challenge assembly and comparison
   without semantic model cost. Human-review records must remain distinguishable
   from model suggestions even in these synthetic checks. Compare recorded
   control verdicts to sealed labels and reconcile cost observations here.
3. **Fixture qualification.** Use actual private control, producer validation,
   publication, and provider capture. A mocked counter-only result is cheaper
   but cannot establish the claimed precondition. Qualification remains inside
   U3 after U1/U2; this oracle-cost ranking does not reorder implementation units.
4. **Provider boundary.** Use controlled responses, tool IDs, errors, canaries,
   and declared limit edges to check optional-mode mechanics before live calls.
   No multi-provider abstraction is required by this property set.
   Reuse `DiagnosticSink` for bounded private diagnostics where applicable;
   truncation cannot pass as a complete semantic capture. Use the JSON publisher
   with explicit `mode: 0o600`, and exclude credentials before writing captures,
   config, or logs. Keep the live command separate from the incident runner and
   preserve that runner's offline endpoint/credential guards.
5. **Reviewed semantic batch.** Only authorized real capture plus human review
   can satisfy the required situation. A reviewed failing batch is still useful
   reachability evidence; it remains a fidelity failure.

Send each record and these fault requirements to `/testing:test-strategy` for
test-form and oracle decisions. Source inspection alone cannot choose an
appropriate real-provider budget or adjudicate meaning. No simulation, test
suite, or reviewer approval was created here. These refinements consume the
supplied fresh review; main portfolio disposition remains with its owner.

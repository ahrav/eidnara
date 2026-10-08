# Compression fidelity: fixture qualification and evaluation

## Scope and evidence boundary

Date: 2026-09-19. Repository: `ahrav/eidnara`.
Inspected HEAD: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
Method: [property catalog contract](../../METHOD.md).

The user supplied the original compression fidelity plan as the evidence scope,
together with local code, docs, and history. No additional
incidents or related repositories were supplied or consulted. This records the
supplied scope, not a general prohibition on seeking evidence.
The published [settled contract](https://github.com/ahrav/eidnara/issues/707)
records the U3 control-qualification and U4 obligations in "Implementation
Decisions", "Milestone boundaries and dependencies", "Evidence and review
gate", and "Resource and security invariants". It is not implementation
evidence. The original plan's SHA-256 is
`b33aaf8508174d39ea578cb6927d1524bbee1c3f2496c8a22511349beb35a384`.
Plan line citations below are historical provenance, not issue offsets. The
[catalog index](../README.md#scope-and-provenance) records the original
title/date, source-section map, and publication evidence.

The plan names inspection revision
`1555f00c702296406186f8c859045014e8e8a4d9`. HEAD advanced during parallel work.
The delta affects curator source/tests and `docs/curator-operations.md`, not
the fixture, provider, incident-pool, or CI sources cited here. References
below were checked against actual HEAD, not copied from the plan's old line
numbers. Historical property catalogs are leads only.

This part covers corpus-byte identity, initial script qualification, evidence
ownership, complete comparisons, control discrimination, cost completeness,
human review, semantic-batch reachability, and bounded live capture. Other parts
own corpus annotations and producer semantics,
served-context meaning, and native/search capability correctness. This part
consumes their observations without reimplementing their oracles.

The planned corpus, Rust fidelity test module, U3 fidelity E2E test, and U4
evaluation script do not exist at this HEAD. No evaluator, fixture, or test was
executed during discovery. Every guarantee below is a **claim under test**.
Confidence describes evidence for the obligation and identified boundary, not
proof that an implementation satisfies it. No new framework, numeric budget,
provider adapter, or production constraint is proposed.

## System model and lens summary

| Focus | Verified model or preserved lead |
| --- | --- |
| Architecture and dependencies | The direct-host fixture supplies producer ModelExecution; the Messages mock captures OpenCode consumer requests. They are separate boundaries. The mock has no record-and-forward mode. |
| State, persistence, and concurrency | Fixture behavior resets to `Success` on consumption. Mock captures are cleared by `reset`. `DiagnosticSink` supplies bounded private diagnostic writes; JSON publication needs an explicit `mode: 0o600`. Neither proves complete capture retention or crash durability. |
| Safety and liveness claims | KTD1/KTD7 require one byte identity and one evidence assembler. U3 requires qualified publication. U4 requires bounded capture and human review. No executing semantic evaluator or successful-completion deadline was found. |
| History and existing checks | Local commit `de84c0d0` narrows incident result lanes to green. Current report validators still expose broader comparison terminology. Old evaluator counts and CI claims are not imported. |
| Failure and degradation | Mock queue exhaustion can use a configured default. Incident reports distinguish run health, behavioral verdict, and baseline comparison; their policy is not the fidelity acceptance policy. |
| Product and assumptions | A smaller request, a successful backend call, or safe abstention cannot independently establish retained meaning. A selected E2E file can pass its gate check without exercising a fold. |
| System wildcard, last | Prebuilt fixture selection checks path existence, so a stale compiled corpus needs an explicit digest witness. Parsed request objects and completion timestamps do not prove raw request/response identity or delivery. |
| Property integrity, replay, and compatibility | Compare full corpus bytes, not canonical JSON or incident-specific domain hashes. Preserve prompt hash differences as treatment, and retain all attempts and baseline identities. |
| Property concurrency, recovery, and lifecycle | Capture before resets. Missing, partial, and inconsistent evidence cannot be replaced by a successful derived row. Required semantic-batch reachability is separate from optional live-mode safety. |
| Property protocol, resources, and security | A live capture must bind the actual selected-model request, response, and tool results. Recorded execution mode distinguishes scripts from real outputs. Fixed whole-run limits and complete cost observations are separate obligations; the default zero-outbound assertion is required. |
| Property distributed coordination | No consensus or replica protocol is in scope. Remote response loss requires per-attempt reconciliation, not an exactly-once provider-effect claim. |
| Property wildcard, last | Reviewed real failures can satisfy batch reachability while failing fidelity. An offline all-green report cannot satisfy it. Recorded control verdicts must match independent sealed labels, including positive controls; deterministic comparison is not semantic proof. |

Both lens sets ran with their wildcard last. The non-durable method-note path is
`/tmp/opencode/compression-fidelity-evaluation-lenses.md`; it is not a durable
dependency. Repeated lenses inspect correlated sources and do not increase
confidence by vote. [Existing checks](existing-checks.md) and the
[fault map](fault-map.md) retain concrete source anchors and missing witnesses.
The fresh review supplied two missing safety surfaces, now recorded as control
discrimination and cost completeness. Main portfolio disposition is maintained
outside this part. Working lens material stays outside repository docs.

## Reachability and index

Reachability is assigned per record. `test-only` describes the existing fixture
and proposed evaluation/test boundaries. The evaluator and forwarding paths are
absent at HEAD, so no existing configured production path is claimed. Future
real capture remains opt-in under the plan; that configuration requirement is
separate from the reachability classification.

| Slug | Type | Reachability | Semantics | Exercise |
| --- | --- | --- | --- | --- |
| [cf-corpus-byte-identity](#cf-corpus-byte-identity) | safety | test-only | always | partial |
| [cf-fixture-script-qualification](#cf-fixture-script-qualification) | safety | test-only | always | partial |
| [cf-authoritative-evidence-assembly](#cf-authoritative-evidence-assembly) | safety | test-only | always | not yet |
| [cf-complete-independent-comparison](#cf-complete-independent-comparison) | safety | test-only | always | not yet |
| [cf-semantic-control-discrimination](#cf-semantic-control-discrimination) | safety | test-only | always | not yet |
| [cf-evaluation-cost-completeness](#cf-evaluation-cost-completeness) | safety | test-only | always | not yet |
| [cf-reviewed-semantic-batch-reached](#cf-reviewed-semantic-batch-reached) | reachability | test-only | sometimes | not yet |
| [cf-bounded-record-and-forward](#cf-bounded-record-and-forward) | safety | test-only | always-or-unreached | not yet |

## Records

### cf-corpus-byte-identity

Type: safety
Reachability: test-only
Status: active
Exercised: partial - `crates/daemon/src/compression_fidelity_tests.rs` pins
the compiled corpus digest and
`packages/e2e-tests/src/compression-fidelity/corpus.test.ts` pins the same
digest at runtime; both reject stale, whitespace-only, same-length, and
scenario-only copies. The run manifest digest does not exist yet.
Guarantee: Every admitted replay or comparison binds Rust and TypeScript to the
same complete corpus file bytes through one SHA-256 identity.
Check: `always` - before admitting evidence, require a readable corpus, compute
SHA-256 over its exact bytes without canonicalization, and require equality with
the Rust compiled-corpus digest, TypeScript runtime-corpus digest, and manifest
digest; a missing or unreadable corpus or any mismatch rejects the run, because
every admitted run needs verifiable identity.
Fault/timing angle: A corpus changes after fixture compilation or between
baseline and candidate loading.
Required faults and enabling state: Load a stale compiled fixture against a
changed corpus, including a whitespace-only edit, a same-length source edit,
and a scenario edit; also construct missing and unreadable input and retain an
unchanged-byte control.
Confidence: high - [evidence](evidence/cf-corpus-byte-identity.md). Plan KTD1
defines raw-byte identity; inspected hashing prior art uses different domains.
Existing check: the two U1 pins above (#718), unaudited; incident
fingerprint tests at `packages/e2e-tests/src/incident-pool/runner.test.ts:248-297`
are related prior art, status `unaudited`.
Impact: Baseline and candidate can appear comparable while using different
sources, scenarios, or compiled fixtures.
Open questions:
- How will the Rust fixture expose its compiled digest to the U4 evaluator?
  U1 pins the same constant in both languages and the TypeScript test reads
  the Rust pin; a runtime exchange is still unimplemented.

### cf-fixture-script-qualification

Type: safety
Reachability: test-only
Status: active
Exercised: partial - the fixture's `script-cases` queue (at most 8 entries)
binds approved examples to presented ordinals, and
`packages/e2e-tests/src/rust-runner/hermetic-host.test.ts` constructs a valid
binding, unknown IDs, an over-bound and an oversized selection, a mismatched
request, and queue exhaustion. `packages/e2e-tests/tests/compression-fidelity-qualification.test.ts`
qualifies one case (C1.S1) through ungated publication and a correlated
provider capture. An identity-bound receipt consumed by later delivery rows
does not exist yet; each row re-judges its own capture.
Guarantee: Initial ungated U3 fixture qualification after U1/U2 requires actual
nonempty publication and correlated provider capture, with bounded request-matched
case scripts before downstream use.
Check: `always` - script selection accepts only declared case IDs within its
declared queue bound, binds aliases to the actual producer request, and records
each consumption; mismatch or exhaustion is explicit failure with no sentinel
success fallback, and qualification is true only with matching consumption,
accepted nonempty publication, and provider-capture observations from the
required ungated lane, because backend success alone is not initial qualification.
Fault/timing angle: An unrelated request consumes the next script, a script is
exhausted, or readiness/completion precedes accepted history publication.
Required faults and enabling state: Construct a valid alias-bound script,
unknown case ID, wrong request aliases, queue exhaustion, and unavailable fold
prerequisites; observe publication and the later provider request separately.
Confidence: high - [evidence](evidence/cf-fixture-script-qualification.md).
Before the case script, `ControlledBackend.execute` answered every summarizer
request with segments over its own presented lines and had no case selection.
Existing check: `packages/e2e-tests/src/rust-runner/hermetic-host.test.ts:121-365`
and `:557-584` check the existing control contract;
`packages/e2e-tests/src/rust-runner/hermetic-host.test.ts:367-555` checks the
case-script queue binding, mismatch, and exhaustion; and
`packages/e2e-tests/tests/compression-fidelity-qualification.test.ts:53-186`
qualifies one case through ungated publication and a correlated provider
capture. Producer counters and gated fold checks are inventoried separately.
All are `unaudited`.
Impact: U3 and dependent U4 results can receive coverage credit without ever
publishing the intended summary.
Open questions:
- Resolved: valid scripted XML reaches nonempty publication through the
  ungated fixture lane without broader runtime work; the qualification test
  executes it.
- The queue bound is 8 entries, the fixture's `MAX_QUEUE`. A reviewer may
  still choose a different bound. (needs human input)

### cf-authoritative-evidence-assembly

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - the U4 assembler and cross-owner observation references
are absent.
Guarantee: Evaluation conclusions derive from intact owner captures assembled
once, and no derived index overrides missing or contradictory captured evidence.
Check: `always` - for every reported conclusion, resolve its case/scenario and
attempt to the owning observation, verify its recorded SHA-256 against retained
bytes, and recompute the conclusion from that observation; missing, hash-invalid,
or inconsistent captures preclude acceptance, while the single run manifest
references each stored observation instead of creating another authority,
because all reports depend on those captures.
Fault/timing angle: Capture resets, interrupted artifact publication, stale
comparison tables, or a capture copied from another attempt.
Required faults and enabling state: Preserve captures before a scripted reset;
then remove, corrupt, cross-bind, or contradict an observation, and leave only a
temporary JSON file without a final publication.
Confidence: high - [evidence](evidence/cf-authoritative-evidence-assembly.md).
Plan KTD7 assigns authority; inspected reset and publisher code establish reuse
hazards without claiming an implemented assembler.
Existing check: `packages/e2e-tests/src/incident-pool/report.ts:396-521` validates
incident row completeness; `runner.test.ts:1039-1054,1074-1094` checks publication
and tampered metadata. Status `unaudited`; no fidelity capture-authority check
was found.
Impact: An internally consistent table can report preservation or recovery that
the actual invocation never supplied.
Open questions:
- Which concrete U2/U3 artifacts expose publication, effective tiers, final
  invocation, and tool outcomes? Their schema is not implemented.
- How will U4 wire the existing publisher with explicit `mode: 0o600` and reuse
  bounded private diagnostic handling without retaining credentials? Integration
  and capture-completeness checks are absent.

### cf-complete-independent-comparison

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - no comparison assembler, complete attempt ledger, or human
semantic-review evidence exists for this corpus.
Guarantee: An accepted comparison covers the same corpus and scenarios on both
sides, retains every attempt and outcome, and uses independent human judgments
rather than model verdicts or rewritten baseline expectations.
Check: `always` - reconcile configured case/scenario/attempt identities with
terminal rows and actual retry/fallback observations; require no missing,
duplicate, or silently discarded attempt, equal corpus/scenario identity,
declared prompt/model/provider/settings/revision identities, observation-backed
scripted versus real execution mode, separate desired and observed baseline
outcomes, control discrimination, cost completeness, and the required human
review; any required unexercised path, model error, missing evidence, unreviewed
result, unresolved
dispute, or fidelity blocker keeps acceptance false regardless of savings,
because every accepted comparison carries these obligations.
Fault/timing angle: Failures are dropped during retries, one side changes its
scenario set, or a model verdict is adopted before human review.
Required faults and enabling state: Supply mixed successful, failed, skipped,
raw-pass-through, and unreviewed attempts; add baseline drift, an unresolved
dispute, and a structurally valid but semantically wrong real output.
Confidence: high - [evidence](evidence/cf-complete-independent-comparison.md).
Plan U4 and acceptance gates specify these distinctions; incident result types
provide partial prior art, not the required semantic policy.
Existing check: `packages/e2e-tests/src/incident-pool/report.ts:207-263,396-448`
and `runner.test.ts:1015-1037,1096-1229` cover incident completeness and outcomes,
status `unaudited`; no independent fidelity-review check was found.
Impact: Best-of-N selection, baseline relabeling, or infrastructure failures can
masquerade as a fidelity improvement.
Open questions:
- How will human approvals and disputes be bound to the exact captured batch?
  The plan defines authority but no evidence representation.
- Which provider/model/settings and predeclared limits define the first real
  comparison? (needs human input)

### cf-semantic-control-discrimination

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - the evaluator and its complete recorded control verdicts
are absent.
Guarantee: A batch qualifies for reviewable semantic comparison only when all
required control verdicts match independent labels sealed before candidate output.
Check: `always` - before marking a comparison reviewable, require exactly the
declared control identities and a recorded verdict for each, then compare every
verdict with its unchanged human-authored expected label; reject missing,
duplicate, altered-label, or mismatched controls, including known-bad samples and
acceptable paraphrase/successful-deployment controls, because an always-accept
or always-abstain shortcut cannot discriminate the required cases.
Fault/timing angle: Expected labels are edited after seeing output, control
verdicts are omitted, or one constant judgment is applied to every sample.
Required faults and enabling state: Present reversed negation, planned-to-completed,
inferred-to-observed, wrong source identity, and lost-hint-qualifier controls
alongside acceptable paraphrases and confirmed deployment; challenge the recorded
verdicts with constant, missing, duplicate, and mismatched results.
Confidence: high - [evidence](evidence/cf-semantic-control-discrimination.md).
Plan U4 scenario 2 requires both bad and acceptable samples; independent source
obligations and human review remain the authority.
Existing check: None found for fidelity control discrimination. Incident
verdict consistency checks do not compare these semantic control labels.
Impact: An evaluator that accepts everything or refuses every answer can appear
qualified despite failing to distinguish the corpus controls.
Open questions:
- How will U4 bind the complete control verdict set to the sealed U1 labels and
  captured batch? No implementation exists to inspect.

### cf-evaluation-cost-completeness

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - no fidelity run supplies the required complete cost
observations or fixed-limit accounting.
Guarantee: Evaluation reports complete observed serving, generation, transform,
and recovery costs for every attempt without hiding uncertainty or changing limits.
Check: `always` - reconcile each configured attempt and actual dispatch with
serving bytes/tokens and estimator/model identity, generation input/output and
attempt count, transform time, recovery calls/output, and attributed provider
cost; separate cold rematerialization, warm repeats, and repeated raw-source
leakage, retain ambiguous attempts with unresolved usage/cost rather than zero,
and withhold cost-complete status while required observations are missing;
require call/output/time/spend bounds declared before execution and unchanged
after failures, because reported savings and authorization depend on these facts.
Fault/timing angle: A timeout loses usage, a retry resets accounting, warm results
hide cold work, or leaked source bytes are omitted from serving cost.
Required faults and enabling state: Supply cold and warm observations, raw-tail
leakage, generation retries/fallbacks, recovery tool calls, an ambiguous provider
attempt, missing estimator identity, and a proposed post-failure limit increase.
Confidence: high - [evidence](evidence/cf-evaluation-cost-completeness.md).
Plan lines 271,278-285 require these observations and fixed limits; mock counters
and zero prices are not actual provider-cost evidence.
Existing check: None found for complete fidelity cost accounting. Mock usage
guards at `packages/e2e-tests/src/mock-provider/server.ts:287-294` are `unaudited`
and enforce only scripted-response shape.
Impact: A comparison can hide wasted attempts, overstate savings, or spend beyond
the approved run limits while showing a complete cost result.
Open questions:
- Which usage/pricing evidence accounts for ambiguous provider attempts without
  fabricating actual cost? The accounting implementation is absent.
- Which estimator/model identities and frozen limit values define the first
  comparison? (needs human input)

### cf-reviewed-semantic-batch-reached

Type: reachability
Reachability: test-only
Status: active
Exercised: not yet - real capture, qualified fold observations, and human review
have not been produced.
Guarantee: Full fidelity acceptance requires a real-output batch that reaches
the required scenarios and receives the specified human review.
Check: `sometimes` - after initial U3 qualification, witness within the acceptance
campaign a batch covering all six cases and required serving scenarios with
actual real-producer outputs,
qualified publication/final-provider observations, completed required consumer
tool loops, and recorded human review of those artifacts; semantic failures
may be present in the witnessed batch, because this checks the evaluation
situation rather than a favorable verdict or mere entry into a command.
Fault/timing angle: Deterministic success, an ignored capture entrypoint, a
skipped fold, or absent provider/reviewer access is mistaken for semantic work.
Required faults and enabling state: Explicit provider authorization and reviewed
synthetic inputs, U2/U3 observations, real outputs, required completed loops,
and human reviewers must all exist in the same identity-bound batch.
Confidence: high - [evidence](evidence/cf-reviewed-semantic-batch-reached.md).
Plan lines 232-242 and 266-271 require this gate; existing scripts and CI do not
supply it.
Existing check: None found. The gated fold E2E test and incident fake-child
report tests are not a real semantic batch.
Impact: A fully offline or unreviewed exercise can falsely certify model meaning.
Open questions:
- Who supplies authorized provider access and the initial two-person review?
  (needs human input)
- Does U3 qualification expose every required fold scenario? Missing scenarios
  block full acceptance even if other rows can be evaluated.

### cf-bounded-record-and-forward

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - the optional forwarding branch and its limit/security
checks are absent.
Guarantee: Optional live evaluation forwards only authorized synthetic requests
to the selected HTTPS provider, returns matching responses through the real tool
loop, excludes credentials from artifacts, and stays within frozen run limits.
Check: `always-or-unreached` - enabled forwarding requires an explicit HTTPS
target and reviewed synthetic data, preserves the actual selected-model request
and matching response/tool-result chain, and records forward execution separately
from scripted execution; the modes are exclusive with no silent matcher/default
fallback, credentials enter only outbound and appear in no capture/config/log
artifact, private JSON publication uses explicit `mode: 0o600`, and the loop
consumes the unchanged limits from `cf-evaluation-cost-completeness`; exhaustion,
unsupported shape, or unfinished loops is unreviewable, because only the enabled
live branch may be unreached, not the separately required default-mode assertion.
Fault/timing angle: Credential capture, post-capture model substitution, provider
failure after send, missing tool results, or retries that reset limit accounting.
Required faults and enabling state: Exercise the disabled branch, an explicitly
configured forwarding branch with canary credentials, request/response and tool
correlation, and each limit or failure boundary using controlled provider input.
Confidence: high - [evidence](evidence/cf-bounded-record-and-forward.md). Plan
U4 supplies the claim; inspected mock/harness code supplies only offline seams.
Existing check: None found for forwarding. Related fixture redaction and
incident isolation checks are `unaudited`; neither establishes live safety.
Impact: Live evaluation can leak credentials, spend beyond authorization, or
judge an invocation different from the one the agent actually received.
Open questions:
- How will the selected real model enter OpenCode before capture, given the
  harness's hardcoded `mock-sonnet` selection?
- How will recorded mode and actual dispatch evidence prevent scripted responses
  from acquiring a real-model label? The model field alone is forgeable.

## Relationships and handoff

- `cf-corpus-byte-identity` constrains all other records. Equal digests do not
  prove semantic correctness or that any scenario ran.
- `cf-fixture-script-qualification` owns initial ungated fixture qualification
  within U3, after U1 and U2. It does not reorder U1-U5. The downstream
  [per-invocation delivery-credit check](../delivery/catalog.md#cf-delivery-credit-requires-published-folded-capture),
  [delivery reachability](../delivery/catalog.md#cf-delivery-scenarios-reach-qualified-invocations),
  [recovery-credit check](../recovery/catalog.md#cf-unavailable-evidence-no-credit),
  and `cf-reviewed-semantic-batch-reached` consume qualification but still need
  their own actual invocation observations. Initial qualification is not blanket
  credit for later delivery or recovery.
- `cf-authoritative-evidence-assembly` supplies trustworthy inputs to
  `cf-complete-independent-comparison`; neither implies the other. Intact
  captures can be incomplete, and complete rows can cite the wrong captures.
- `cf-bounded-record-and-forward` constrains optional live execution.
  The separate `always` assertion `cf-eval-offline-zero-outbound` is required
  whenever scripted/default mode runs, with required default-branch coverage.
  Live evaluation remains a separate command from the offline incident runner;
  incident endpoint and credential guards must not be relaxed.
  `cf-reviewed-semantic-batch-reached` prevents its absence from passing the
  required acceptance campaign. A reviewed failing batch reaches the latter
  while remaining rejected by `cf-complete-independent-comparison`.
- `cf-semantic-control-discrimination` compares recorded verdicts with sealed
  labels from the [source-obligation owner](../generation/catalog.md#cf-source-obligation-independence).
  This is a deterministic consistency check, not proof that semantic labels or
  judgments are correct. Human review remains required. Blinding comparison-arm
  labels is an optional review recommendation, not an acceptance requirement.
- `cf-evaluation-cost-completeness` owns cost observations and fixed-limit
  completeness; forwarding consumes its whole-loop limits. Cost observations
  share authoritative capture ownership with fidelity observations. No measured
  latency baseline, percentage threshold, or reliability rate is invented.

Route each active record to `/testing:test-strategy` with its evidence file.
Identity, assembly, comparison, controls, and costs need deterministic artifact
checks before live work. This is routing guidance, not a new implementation-unit
order. Qualification and forwarding need the existing fixture/provider
boundaries. Batch reachability needs authorized capture and human review, not
a larger mocked suite. Route existing-test adequacy to
`/testing:invariant-test-review` and runtime guard strength to
`/low-level-systems:defensive-assertions-and-invariant-guards`. Use
`/testing:deterministic-simulation-testing` only if the selected strategy needs
seeded scheduling; discovery does not choose a new harness.

There are eight active records: seven safety and one reachability. Record
semantics are six `always`, one `always-or-unreached`, and one `sometimes`.
The separately named default-mode assertion remains required. No formal liveness
claim is inferred from bounded evaluation. Fresh-review refinements are applied
here; main disposition remains with the portfolio owner.

Mechanical verification confirms eight index rows, eight schema-ordered records,
and eight evidence files of 99-114 lines. All 129 relative links resolve, and
the 19 linked source files match the U2 branch tip. The fault map names four
safety assertions and 29 occurrence markers as separate kebab-case constants.
These checks validate documentation structure and references, not property
exercise, semantic truth, or existing-test adequacy.

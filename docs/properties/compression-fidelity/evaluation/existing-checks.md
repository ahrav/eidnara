# Evaluation existing-check inventory

Date: 2026-09-19. Repository: `ahrav/eidnara`.
Inspected HEAD: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
Scope and the supplied plan's provenance are in [the catalog](catalog.md).
Only local code, docs, and history were consulted. No extra incidents or related
repositories were consulted, and no tests were executed.

Every listed check is `unaudited`. Inspection establishes its location and
assertion, not its adequacy or a passing run. Related checks are not direct
coverage of the planned compression evaluator. Paths below are repository-root
relative and were checked against HEAD.

## Corpus identity

| Check and location | Condition or diagnostic | Status |
| --- | --- | --- |
| `semanticFingerprint`, `packages/e2e-tests/src/incident-pool/registry.ts:136-148`; `history.ts:29-39` | Hashes canonical structured meaning with an incident contract tag, not the raw corpus file. | unaudited |
| Fingerprint tests, `packages/e2e-tests/src/incident-pool/runner.test.ts:248-274` | Formatting/key order preserve the fingerprint; changes to owning semantic fields alter it. | unaudited |
| `implementationBundleDigest`, `packages/e2e-tests/src/incident-pool/registry.ts:152-169`; tests at `runner.test.ts:276-290` | Hashes domain tag, sorted paths, lengths, and bytes; rejects non-root-confined or empty file lists. This is not plain corpus SHA-256. | unaudited |
| Ledger fingerprint test, `packages/e2e-tests/src/incident-pool/runner.test.ts:293-297` | Appending a baseline event changes the ledger fingerprint. | unaudited |

No shared Rust/TypeScript compression-corpus digest check was found. The planned
corpus is absent. `crates/daemon/Cargo.toml:58` already includes `sha2`; dependency
availability is not cross-language identity evidence. A 64-character digest
shape check in `report.ts:145-150` does not verify artifact content. No
compression-corpus missing-file or read-failure rejection check was found.

## Fixture and U3 qualification

| Check and location | Condition or diagnostic | Status |
| --- | --- | --- |
| Rust control framing, `crates/daemon/examples/direct_host_fixture.rs:604-634,673-729,749-832` | Rejects unknown fields/commands and oversized lines; emits static `control request rejected` diagnostics. Request and response frames have a 64 KiB cap. | unaudited |
| Readiness parser and publication check, `packages/e2e-tests/src/rust-runner/hermetic-host.ts:345-404` | Checks readiness keys, wire version/catalog, and owner/mode; rejects `direct host readiness preceded secure publication`. | unaudited |
| Readiness and stale PID tests, `packages/e2e-tests/src/rust-runner/hermetic-host.test.ts:105-189` | Rejects unexpected readiness fields, oversized records, early publication, and checks PID-age boundaries. | unaudited |
| Control-response test, `packages/e2e-tests/src/rust-runner/hermetic-host.test.ts:191-223` | Malformed, unknown-field, oversized, wrong-ID, and duplicate replies fail the client. | unaudited |
| Real fixture contract test, `packages/e2e-tests/src/rust-runner/hermetic-host.test.ts:225-349` | Checks owner-only paths, unchanged counters after malformed control, success/block/release/failure counts, absence of secret/sentinel renderings, and teardown. It is prerequisite-gated. | unaudited |
| SIGTERM test, `packages/e2e-tests/src/rust-runner/hermetic-host.test.ts:351-378` | Checks control/publication removal and teardown after terminating a blocked fixture. It is prerequisite-gated. | unaudited |
| Producer route test, `packages/e2e-tests/tests/rust-history_summarizer-producer.test.ts:82-90` | Requires started/completed counters at least one; it does not assert accepted nonempty history. | unaudited |
| Producer failure test, `packages/e2e-tests/tests/rust-history_summarizer-producer.test.ts:92-110` | Requires one additional backend failure, matching failure text, and positive backoff time. | unaudited |
| Fold gate check, `packages/e2e-tests/tests/rust-fold-under-pressure.test.ts:15-21` | When the live fold scenario is disabled, asserts `foldInfraEnabled()` is false. That is gate evidence, not fold coverage. | unaudited |
| Fold scenario, `packages/e2e-tests/tests/rust-fold-under-pressure.test.ts:42-90` | Looks for `HARD`, reduced wire size, and nonempty session history; requires `EIDNARA_E2E_FOLD=1` plus prerequisites. | unaudited |

No case-ID script control, alias-matching script queue, consumed-script count,
or ungated compression-fidelity publication qualification check was found.
`ControlledBackend.execute` accepts `_request` without inspection and returns
`fixture-success` on success (`direct_host_fixture.rs:383-499`). A completion
counter therefore cannot substitute for the U3 publication witness.
This inventory concerns initial ungated qualification inside U3 after U1/U2.
Downstream delivery and recovery checks still require their own per-invocation
evidence; initial qualification does not grant permanent coverage credit.

## Provider observations and tool loop

| Check and location | Condition or diagnostic | Status |
| --- | --- | --- |
| Mock dispatch, `packages/e2e-tests/src/mock-provider/server.ts:262-302` | Uses matcher, queue, then configured default; without any response returns HTTP 500, `No scripted response available`. | unaudited |
| Mock usage guard, `packages/e2e-tests/src/mock-provider/server.ts:402-424` | Scripted errors bypass usage; missing both produces HTTP 500, ``MockResponse requires `usage` or `error` ``. | unaudited |
| Scripted model assignment, `packages/e2e-tests/src/mock-provider/server.ts:428-429` | A script can set the response model string; otherwise it echoes the request model. This is not real-execution provenance. No script/forward exclusivity check exists. | none found |
| Tool publication guard, `packages/e2e-tests/src/scripted-tool-call.ts:120-139` | Throws when the requested tool was never published in the provider request. | unaudited |
| Tool-result guard, `packages/e2e-tests/src/scripted-tool-call.ts:62-77,140-145` | Finds a provider-visible `tool_result` by `tool_use_id`; missing result is an infrastructure error. Returned text is a projection, not raw result bytes. | unaudited |
| Prompt error handling, `packages/e2e-tests/src/rust-harness.ts:375-406` | Timeout throws and SDK rejection is promoted to error. The `Promise.race` is not evidence that remote work stops or that cost is capped. | unaudited |

No `src/mock-provider/server.test.ts`, scripted-tool helper unit test, forwarding
mode, real-response capture, whole-loop limit check, or credential-exclusion
check for live fidelity artifacts was found. The mock stores incoming headers
and parsed JSON (`server.ts:221-249`). `reset` discards request history
(`server.ts:169-171`), and the scripted tool helper calls it at line 101.
The required default-mode zero-outbound assertion also has no dedicated fidelity
check. Its requirement is unconditional for default runs, even while future
enabled forwarding remains optional.

## Evidence assembly and outcomes

| Check and location | Condition or diagnostic | Status |
| --- | --- | --- |
| Case-result validation, `packages/e2e-tests/src/incident-pool/report.ts:207-302` | Unhealthy runs must be `not_evaluated` and `unscored`; completed unevaluated rows require a precondition/dependency reason. | unaudited |
| Exact incident schema, `packages/e2e-tests/src/incident-pool/report.ts:304-374` | Requires incident family/variant/baseline IDs, closed fields, digest shapes, and valid outcome combinations. Raw semantic captures do not fit this schema. | unaudited |
| Report construction, `packages/e2e-tests/src/incident-pool/report.ts:396-448` | Rejects empty selection, duplicate/unselected/missing terminal rows, and selected-set digest mismatch. | unaudited |
| Report parsing, `packages/e2e-tests/src/incident-pool/report.ts:453-521` | Recomputes counts, selected-set digest, and evaluation completeness; checks completion marker and exact keys. | unaudited |
| Outcome-policy functions, `packages/e2e-tests/src/incident-pool/report.ts:672-710` | Distinguish incomplete dependencies and scored baseline mismatches. These are incident command rules, not fidelity acceptance rules. | unaudited |
| Selection tests, `packages/e2e-tests/src/incident-pool/runner.test.ts:1015-1037` | Reject empty selection and duplicate/missing/unselected terminal rows. | unaudited |
| Publication tests, `packages/e2e-tests/src/incident-pool/runner.test.ts:1039-1072` | A manually created temporary report does not create a final report; normal/scheduled publication can be parsed. No crash or power-loss fault is injected. | unaudited |
| Tamper tests, `packages/e2e-tests/src/incident-pool/runner.test.ts:1074-1094` | Reject altered completion/evaluation flags, counts, extra fields, and digest-inconsistent rows. | unaudited |
| Unhealthy/dependency/baseline tests, `packages/e2e-tests/src/incident-pool/runner.test.ts:1096-1195` | Keep unhealthy runs unevaluated, require still-blocking dependencies, and distinguish regression from expected green. | unaudited |
| Callback/whole-pool tests, `packages/e2e-tests/src/incident-pool/runner.test.ts:1197-1229` | Keep a static crash row after callback failure, exclude its private diagnostic, and collect the fake-child selected set. | unaudited |

The shared publisher at `packages/e2e-tests/src/atomic-publish.ts:5-16` writes a
temporary file and renames it. There is no dedicated publisher test file, no
file/directory sync, and no crash-durability witness. Its default mode is 0644;
private evaluation JSON must pass explicit `{ mode: 0o600 }`. This is required
integration work, not existing evaluator behavior. Permissions do not redact
credentials; capture, config, and log artifacts must exclude them before writing.

No U4 authoritative-capture assembler, all-attempt comparison, source-bound
human approval check, or required real semantic-batch witness was found.

## Control discrimination and cost completeness

No check was found that compares a complete set of recorded fidelity control
verdicts with independent sealed labels. `report.ts:207-302` checks structural
outcome combinations, not whether known-bad samples fail and positive controls
pass. No implemented control sealing or semantic oracle is claimed here.

No complete fidelity cost check was found. Relevant prior art is narrower:

| Observation or check | Condition or limitation | Status |
| --- | --- | --- |
| `packages/e2e-tests/src/rust-harness.ts:444-453` | Measures normalized messages from the last main request; absent requests return zero/empty output. This is not all-attempt, complete-invocation cost evidence. | unaudited |
| `packages/e2e-tests/src/mock-provider/server.ts:287-294` | Requires scripted usage unless returning an error. It does not measure actual provider charges or ambiguous attempts. | unaudited |
| `packages/e2e-tests/src/opencode-runner/spawn.ts:174-187` | Mock model prices are zero. These configured values cannot substitute for real generation/recovery cost. | unaudited |

The plan requires bytes/tokens with estimator/model identity, generation
input/output and attempts, transform time, recovery calls/output, cold/warm
separation, and repeated-source leakage. No current check combines those facts
or enforces fidelity run-wide frozen call/output/time/spend bounds.

## Related isolation checks, not live-mode qualification

| Check and location | Condition or diagnostic | Status |
| --- | --- | --- |
| Child credential test, `packages/e2e-tests/src/incident-pool/runner.test.ts:804-824` | Strips credential/proxy canaries and relocates HOME/TMPDIR in incident child execution. | unaudited |
| Endpoint tests, `packages/e2e-tests/src/incident-pool/runner.test.ts:826-862` | Reject non-loopback incident endpoints and unsafe override names. They do not authorize or test HTTPS forwarding. | unaudited |
| Environment override test, `packages/e2e-tests/src/incident-pool/runner.test.ts:864-881` | Rejects isolation, identity, credential, and proxy overrides. | unaudited |
| Private workspace creation, `packages/e2e-tests/src/incident-pool/support/case-workspace.ts:21-47` | Creates and chmods the workspace/subdirectories to 0700. This is an existing reuse candidate, not a fidelity artifact implementation. | unaudited |
| `DiagnosticSink`, `packages/e2e-tests/src/incident-pool/support/case-workspace.ts:129-159` | Requires a positive integer byte cap, opens exclusively with mode 0600, caps writes, records truncation, and closes its descriptor. It does not redact secrets or enforce provider-spend limits. | unaudited |
| Sink integration, `packages/e2e-tests/src/incident-pool/runner.ts:733-778` | Validates offline endpoints/overrides before spawning, uses bounded stdout/stderr sinks, closes them, and deletes the workspace. This teardown cannot be copied blindly for retained semantic captures. | unaudited |
| Sink tests, `packages/e2e-tests/src/incident-pool/runner.test.ts:925-979` | Checks diagnostic truncation, byte cap, teardown, exclusion from reports, and static unhealthy results after sink failure. | unaudited |

Reuse bounded private diagnostic handling before adding another limiter, but do
not treat truncated output as complete evidence. Live evaluation is a separate
command from the incident runner. Its implementation must not relax the incident
runner's offline endpoint or credential guards. Credential injection is outbound
only; private file modes are not a substitute for exclusion from artifacts.

## Execution selection and quiet areas

Current package scripts are at `packages/e2e-tests/package.json:6-26`. There is
no `eval:compression-fidelity` command. CI's native-addon job builds prerequisites
and invokes fixture-contract, mode-manifest validation, and Rust E2E at
`.github/workflows/ci.yml:973-1000`. This is selection evidence only; no CI result
is claimed. `rust-scenario-support.ts:12-18` keeps broad fold execution optional.

The historical inventory under `docs/properties/daemon/history_summarizer/`
contains old evaluator labels and explicitly absent source references. Local
tracked-file and filesystem inspection found no `history_summarizer-eval`
implementation at HEAD. No old test count, CI command, or semantic capability
is inherited. The quiet areas are the unbuilt U3 script/publication link, raw
response retention, cross-language identity, control discrimination, complete
cost accounting, bounded live loop, and independent human acceptance. Send
adequacy questions to the owners named in the catalog.

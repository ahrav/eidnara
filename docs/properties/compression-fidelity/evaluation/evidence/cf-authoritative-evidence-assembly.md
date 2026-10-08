# cf-authoritative-evidence-assembly

## Discovery trigger

KTD7 in the [settled contract](https://github.com/ahrav/eidnara/issues/707),
"Implementation Decisions" (historical plan line 159), makes U4 the single
assembler. U2 owns parser/publication/internal source observations;
U3 owns provider captures and tool outcomes. Derived tables
cannot override those captures. This is an obligation, not existing evaluator code.

Inspected HEAD: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`, 2026-09-19.
[Catalog provenance](../catalog.md#scope-and-evidence-boundary) records the
original plan and revision drift. This record is test-only because it concerns
evaluation artifacts outside production serving. Exercise: not yet.

## Evidence trail

- Plan lines 175-179 require stage-specific outcomes; line 271 records run
  identity once and derives tiers, ranges, availability, budgets, and costs
  from owning observations rather than duplicate fields.
- [CapturedRequest](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L66-L80)
  stores parsed body, headers, and timestamps. [Request handling](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L221-L249)
  calls `req.json()` and stores every incoming header, not raw request bytes.
- [Reset](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L161-L171)
  discards captured requests. [Scripted tool execution](../../../../../packages/e2e-tests/src/scripted-tool-call.ts#L93-L117)
  resets before driving the next tool turn. Prior captures need retention first.
- [Tool-result projection](../../../../../packages/e2e-tests/src/scripted-tool-call.ts#L49-L77)
  extracts text by call ID and joins inner blocks. It is useful observation
  machinery, not the complete response/tool artifact.
- [Incident report construction](../../../../../packages/e2e-tests/src/incident-pool/report.ts#L396-L448)
  checks selected rows and recomputes its domain-specific digest. The parser
  at lines 453-521 checks structural consistency. Neither validates U2/U3 captures.
- [Exact incident result schema](../../../../../packages/e2e-tests/src/incident-pool/report.ts#L304-L374)
  requires catalog family/variant/baseline identities and disallows extra fields.
  Raw semantic captures do not belong in that report or publication path.
- [Atomic publisher](../../../../../packages/e2e-tests/src/atomic-publish.ts#L5-L16)
  writes JSON to a random temporary path, using mode 0644 unless supplied, then
  renames it. U4 must pass explicit `{ mode: 0o600 }` for private JSON; this
  integration is not implemented. The helper does not sync file or directory.
- [DiagnosticSink](../../../../../packages/e2e-tests/src/incident-pool/support/case-workspace.ts#L129-L159)
  opens exclusively with mode 0600, caps diagnostic writes, and reports
  truncation. It is existing bounded/private prior art, not a secret filter or
  a complete semantic-capture writer. Its tests at
  [runner.test.ts:925-979](../../../../../packages/e2e-tests/src/incident-pool/runner.test.ts#L925-L979)
  are `unaudited`. Reuse it where applicable before inventing another limiter.
- [Publication test](../../../../../packages/e2e-tests/src/incident-pool/runner.test.ts#L1039-L1054)
  manually creates an orphan temporary file, checks no final file exists, then
  publishes normally. [Tamper tests](../../../../../packages/e2e-tests/src/incident-pool/runner.test.ts#L1074-L1094)
  check metadata consistency. Both are `unaudited`; neither is a power-loss test.

## Failure scenario

A derived row says “P4 preserved” after its owner capture was cleared or replaced
by another attempt. Alternatively, an intact capture contradicts the row but the
assembler prefers the newer table. Counts and selected-set digests still match,
so an incident-style structural validator alone does not detect the false claim.

The competing explanation is that atomic JSON publication establishes reliable
evidence. The publisher only changes final-path visibility; it neither validates
the content nor establishes crash durability. This is a reuse boundary, not an
allegation of an observed evaluator data-loss incident.

## Timing windows and dependencies

Capture precedes helper reset and artifact derivation. Publication of the final
manifest follows its observations; a temporary file alone is not a completed
manifest. If required artifacts are missing or inconsistent after interruption,
the result is incomplete rather than reconstructed from a successful index.

Corpus and artifact hashes have different inputs but both use standard SHA-256.
The assembler must distinguish authored/parsed tiers from final provider bytes;
a provider capture cannot establish parser healing on its own. Native source
and search correctness remain with their owning part.
Capture, config, and log artifacts must exclude credentials before publication;
outbound injection is the only credential use authorized by the plan. Diagnostic
truncation must not become a complete capture. The live command remains separate
from the incident runner and preserves its offline guards.

## What a test must construct

1. Retain owner observations and their byte hashes before resetting the provider.
   Derive result rows by case/scenario/attempt references to those observations.
2. Delete, tamper with, cross-bind, or truncate one artifact while leaving a
   favorable derived row intact. Acceptance must remain unavailable.
3. Contradict an intact capture with a derived index. Check that the index never
   overrides captured evidence, without prescribing a new repair mechanism.
4. Leave a temporary JSON file without final publication. Check incomplete
   reporting, not a crash-durability promise.
5. Verify explicit mode 0600 and credential exclusion in capture/config/log
   artifacts. A private file containing a credential still violates the contract.

## Investigation log

### Q: What concrete observation schemas will U2/U3 expose?
- Sources examined: Plan lines 159,175-179,271; current capture and report types.
- Findings: Owners and required observations are named; the fidelity test module,
  U3 test, and assembler are absent. Existing incident row schemas are narrower.
- Missing evidence: Implemented parser/publication/provider/tool/source artifacts.
- Conclusion: Unresolved, needs U2/U3/U4 implementation. Keep the single-owner
  requirement without inventing another evidence schema or storage service.

### Q: Does the shared publisher guarantee private, crash-durable artifacts?
- Sources examined: `atomic-publish.ts:5-16`, `runner.test.ts:925-979,1039-1054`,
  and `support/case-workspace.ts:129-159`.
- Findings: It defaults to 0644 and uses write-plus-rename without sync. The test
  does not kill a writer. The plan requires private artifacts, not a new
  crash-durability protocol.
- Missing evidence: U4's explicit mode-0600 publication, safe reuse of bounded
  diagnostics, credential exclusion, and retained-artifact tests.
- Conclusion: Resolved that the helper alone supplies neither guarantee;
  unresolved, needs the U4 private-artifact handling. Do not claim durability.

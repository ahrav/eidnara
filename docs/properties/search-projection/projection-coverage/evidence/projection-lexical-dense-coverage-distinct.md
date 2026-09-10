# projection-lexical-dense-coverage-distinct

## Discovery trigger

RP2.1 line 156 explicitly separates lexical coverage from vectors valid for
the current revision/model. The version lens and wildcard find a false-health
scenario: every row has some vector, but none belongs to the active identity.
This record checks report truth, not embedding execution or quality.

## Evidence trail

Provenance: [source register](../_lenses/model.md#source-register), dated
2026-09-10, Eidnara HEAD `913234433ae36a80a6e22c6aac14c7f9aab74386`.
No coverage measurements or inference runs were supplied or performed.

- [RP2.1 failure semantics][failure] retain lexical rows when tokenizer identity
  or exact count is unavailable and mark dense coverage missing.
- [U4][plan] requires class coverage and separate current revision/model vector
  counts. Pending rows are work obligations, not evidence that vectors exist.
- [Requirements][requirements] trigger rebuild on schema, tokenizer, model,
  projection-policy or identity-contract mismatch.
- [Rendered-memory snapshot][memory] holds object/category/content and a
  rendered digest. It does not carry a dense-validity certificate.
- [Existing withheld-read record][withheld] distinguishes unserved canonical
  reads from served-empty reads. Reuse that observability principle without
  claiming it already reports projection coverage.
- [Workspace][workspace] and source searches show no RP2.1 projection report
  or occurrence schema at HEAD.

Reachability: `test-only`. Canonical composition metadata exists, but no
production RP2.1 lexical/dense report consumes occurrence revisions and active
model validity. Lower-level vector execution is owned by the embedding lane
and does not establish this report's reachability.

## Failure scenario

A source row is revised while its old vector remains. A count query joins by
payload or object alone and calls the row dense-covered. A model change produces
the same error across the entire corpus. Deletion can also leave a vector that
inflates a count after its occurrence is no longer current.

A competing explanation is intended lexical-only policy. Raw tool output is
not dense-required by default, so its missing vector is not failed embedding
coverage. The report must distinguish policy exclusion from required-but-missing
work, stale output and unknown inventory. A single percentage cannot encode all
of these states without an explicit denominator and identity.

## Timing windows and dependencies

One report must identify its local checkpoint, source policy and active model.
Its lexical and dense sets must be read coherently rather than sampling on
opposite sides of a revision transition.
The embedding owner defines validity, including revision, model fingerprint,
dimension and current-input hash checks. This part consumes those facts and
does not duplicate token counting, JobTable state or inference validation.
The export/rebuild owner handles mismatched projection compatibility.
The report must not label incompatible state complete while that owner rebuilds.
Where approved input mapping uses a remediated canonical field, the
[remediation record][remediation] adds a current-byte/hash comparison even if
source revision is unchanged. It does not assume every embedding-key field
agrees: a freshly computed input hash may already discriminate the change.

## What a test must construct

1. Independent current-occurrence and dense-required sets for each source class.
2. Full lexical coverage with a mixture of valid, stale, absent and pending
   vector states, plus raw tools intentionally excluded from dense policy.
3. A source revision whose rendered text is unchanged, and a model identity
   switch whose vector dimensions happen to remain equal.
4. A deleted occurrence with a retained old vector and another live occurrence
   sharing its payload bytes.
5. Missing exact-token preflight facts with lexical rows still present.
6. A known empty class and an unknown/unavailable class inventory, ensuring
   their report states are distinguishable without inventing a ratio rule.
7. Direct set equality against the fixture, then derived count consistency.
   Include a dense-required occurrence with no vector and current pending work,
   one with no vector and no pending work, and one with a valid vector whose
   completion bookkeeping still leaves a pending row. Only the first is in
   pending coverage; the first two are missing dense coverage. The last may
   consume pending-job capacity without being missing coverage.
8. Old-vector and same-render/new-revision markers in [fault-map](../fault-map.md).

These report-local states can be supplied as validity fixtures to avoid
retesting inference in this record. The [acceptance witness record][acceptance]
still requires the embedding owner's certified offered-input path; report
fixtures cannot substitute for that required campaign scenario.
No current report check exists; existing withheld-read tests remain unaudited
and are linked rather than duplicated.

## Investigation log

### Q: How do pending work and missing coverage relate?

- Sources examined: [failure semantics][failure], [U4][plan], the catalog
  check, and the fresh final verifier's pending-set warning.
- Findings: Missing coverage already means required minus valid. Pending work
  is not evidence of a valid vector, and a durable vector may precede completion
  bookkeeping. Coverage and raw pending-job capacity are different observations.
- Missing evidence: No runtime report exists or was exercised.
- Conclusion: The proposed reporting contract defines pending coverage as the
  subset of missing occurrences backed by current, non-obsolete durable work.
  This clarifies set semantics without choosing wire fields or a scheduling rule.

### Q: What identifies the report and its missing/zero-denominator states?

- Sources examined: [failure semantics][failure], [U4][plan],
  [requirements][requirements] and [canonical memory][memory].
- Findings: Separate lexical and current-valid dense reporting is explicit.
  Existing composition metadata names a canonical read, not a projection
  policy/model coverage snapshot. The report schema is proposed.
- Missing evidence: Observation identity, expected-set provenance, treatment
  of unknown inventory, zero-denominator representation and embedding validity
  interface from the owning lane.
- Conclusion: needs human input from projection, embedding and RP2.9 owners.
  The invariant fixes the set relationships without choosing new wire fields,
  metric labels or a misleading default percentage.

[failure]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L108-L120
[plan]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L151-L156
[requirements]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L35-L41
[memory]: ../../../../../crates/daemon/src/canonical_memory.rs#L21-L87
[withheld]: ../../../daemon/transform/catalog.md#canonical-read-staleness-is-distinguishable-from-emptiness
[workspace]: ../../../../../Cargo.toml#L3-L17
[remediation]: ../catalog.md#projection-remediation-invalidates-derived-bytes
[acceptance]: ../catalog.md#projection-acceptance-situations-witnessed

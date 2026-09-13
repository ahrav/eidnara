# typed-failure-preserves-tree-outcome

## Discovery trigger

R2 preserves A2 while KTD1 introduces derived duplicate-field errors inside
message and block envelopes. The failure and protocol lenses specialize the
[canonical A2](../../../hot-path-optimization/latency-audit/catalog.md#route-and-typed-decode-are-independent-of-entry-path).

System: `/local/home/ahrav/scratch/eidnara`, 2026-09-13.
HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Plan baseline `e451a2b4` has the same decoder. HEAD's added
`pub mod search_seed;` line shifts the following daemon references by +1.

## Evidence trail

- `crates/daemon/src/lib.rs:12921-12938` gates a direct attempt, returns
  Refused immediately, and restarts on Invalid before falling through.
- `crates/daemon/src/lib.rs:12941-12951` decodes original bytes as Value.
  Invalid JSON becomes Null for normal dispatch classification.
- `crates/daemon/src/lib.rs:8115-8123` maps invalid typed conversion of a
  valid tree to `bad_request`. These two invalid-input classes are distinct.
- `crates/daemon/src/lib.rs:15758-15765,15864-15949` preserves tree checks
  before a derived parser can skip malformed unknown data.
- `crates/daemon/src/lib.rs:19986-20117` compares parsers; `:20140-20181`
  compares body-entry outcomes with tree dispatch. Both checks are unaudited.
- `crates/daemon/src/transform.rs:810-818,917-927` preserves the Option
  serializer_profile wrapper and maps missing/null to the empty typed string.

Reachability is default-production through Handler::handle. A terminal can
also reflect later validation, so decode acceptance and handler result must
be observed separately. BodyLane::Direct does not imply successful serving.

## Failure scenario

Derived serde rejects a repeated recognized CK role or block kind. Returning
that error directly would reject input the old Value path collapses last-wins.
Alternatively, removing the compatibility gate would allow ignored malformed
data that the tree parser rejects.

There is also a two-sided-oracle trap: both final lanes may change acceptance
in the same way. Compare baseline/final classifications as well as the final
lane pair. Apply only accepted R3/default omission normalization to outputs.

## Timing windows and dependencies

The duplicate must occur after a valid prefix in literal bytes. Building a
Value first erases the duplicate before the vulnerable attempt.
Admission is nonbinding for semantic comparisons. Refusal-code tests remain
A1/A3 ownership; Refused is not a parse error eligible for recovery here.
The serde_json raw-value token has position-sensitive behavior under re-read.

## What a test must construct

Freeze literal cases for duplicate ingress mid, CK role, block kind, enum tag,
and duplicate keys inside retained maps. Include missing/explicit-null
serializer_profile, null/defaulted booleans, unknown tags, numeric range,
surrogates, UTF-8, depth, trailing bytes, and positional/wrong-shape envelopes.
Check route, decode classification, known fields, payload Values, and handler
terminal class/code. The existing helper also compares diagnostic messages;
that stronger existing check must be reviewed rather than silently weakened.
Explicit-null serializer_profile preserves the existing wrapper boundary;
this witness does not authorize the deferred TransformRequestWire refactor.

Proposed raw-token witness: put an object with `a` before
`$serde_json::private::RawValue` under a discarded `ck.future` field, with a
non-string token value. Compare baseline/final behavior through the body entry.
No such proposed fixture runs here, and no mechanism-specific bug is claimed.
Any observed raw-token acceptance change requires stop-and-report. No
compatibility exception is authorized. The independent review validates this
as a discriminating requirement, not as a confirmed defect without a run.

## Investigation log

### Q: Can discarded CK fields change acceptance after removing re-reads?

- Sources examined: daemon lib.rs:19735-19741 and memory-store
  lib.rs:131-133,255-257.
- Findings: a later token can parse from bytes and fail on sorted Value
  re-read. Custom envelope serde supplies additional such re-reads.
- Competing explanation: the initial parse or retained payload still rejects
  the body, making removal irrelevant for that placement.
- Missing evidence: the exact placement run on baseline and final source.
- Conclusion: unresolved, needs a discriminating baseline/final fixture.

### Q: What is the disposition if that fixture changes admission?

- Sources examined: settled R2, R3, A2, and the explicit user amendment
  supplied with the independent review on 2026-09-13.
- Findings: unknown-field discard is accepted, broader admission drift is not.
- Missing evidence: the required baseline/final run remains absent.
- Conclusion: resolved with answer: stop and report any observed acceptance
  change. No compatibility exception is authorized; R3 remains accepted.

### Q: Are diagnostic strings stable beyond error codes?

- Sources examined: daemon lib.rs:20121-20131 and host contract line 354.
- Findings: the existing differential compares messages; the host's diagnostic
  rule there applies to controls, not automatically to CK application errors.
- Missing evidence: an application-body decision permitting diagnostic drift.
- Conclusion: unresolved, preserve the existing comparison pending evidence.

## Named handoff

`/testing:test-strategy` owns the frozen corpus and highest useful entry seam.
`/testing:invariant-test-review` owns A2 oracle adequacy. Independent analyst
`ses_f6756093fffeVjNp36S3E8pKrM` completes the supplied portfolio pass on
2026-09-13; its disposition retains the raw-token hypothesis without adopting
another reviewer's unrun defect claim. The implementation owner enforces
stop-and-report. No tests run; this property remains unexercised.

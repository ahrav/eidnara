# prepared-field-output-and-audit-policy-agree

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

Moving prepared text instead of cloning it must preserve the policy result and
the metadata used to describe that result in durable audit rows.

## Evidence trail

- [memory-store/lib.rs:2204-2243][prepare] selects the scanner by layer, refuses
  detected NewIdentity input, chooses output, checks its bound, then appends.
- [2350-2424][audit] writes detector provenance, detection count/labels, field
  owners, and the policy action without reading retained redacted text.
- [3428-3494][units] applies durable and transaction preparation to core units.
  Production callers include [8306][durable-call] and [8921-8931][transaction-call].
- [18171-18212][test] compares preserved identity output with substituted content
  and their persisted actions. It remains unaudited.

## Failure scenario

An ownership refactor drains detections along with text or swaps policy branches.
The durable value may be correct while its receipt claims the wrong action.
A secret NewIdentity must refuse, not produce a successful rejected-detection batch.

## Timing windows and dependencies

The complete policy matrix is explicit; detected input means the selected layer
reports a detection. Clean input succeeds in every row subject to byte bounds.

| Layer | Policy | Output and audit behavior for detected input |
| --- | --- | --- |
| Durable | Content | It substitutes text and records substitute actions. |
| Durable | NewIdentity | It refuses before scan append. |
| Durable | ExistingIdentity | It preserves input and records preserve actions. |
| Transaction | Content | It substitutes text and records substitute actions. |
| Transaction | NewIdentity | It refuses before scan append. |
| Transaction | ExistingIdentity | It preserves input and records preserve actions. |

## What a test must construct

Compare output/refusal, detections, field IDs, owner relationships, and actions
for all six pairs with clean and detected fixtures. An applied clean NewIdentity
has a zero-finding field_scans row and owner copies, with no scan_detections row,
even though its preparation policy action is named reject. Preserve detector
build provenance and normalize only generated audit identifiers/timestamps.
No six-policy experiment runs here; this record is not exercised.

## Investigation log

### Q: Which inputs independently exercise the two scan layers?

- Sources examined: [Layer dispatch][prepare] and [core-unit preparation][units].
- Findings: Both layers use the same policy branches but select different scan
  entrypoints. One receipt example does not cover all six combinations.
- Missing evidence: Independent detector fixture expectations for each layer
  and a complete policy matrix run are not supplied.
- Conclusion: The matrix and output rules are resolved; fixture selection
  remains unresolved for `/testing:test-strategy`.

[prepare]: ../../../../crates/memory-store/src/lib.rs#L2198-L2237
[audit]: ../../../../crates/memory-store/src/lib.rs#L2344-L2418
[units]: ../../../../crates/memory-store/src/lib.rs#L3459-L3525
[durable-call]: ../../../../crates/memory-store/src/lib.rs#L8496
[transaction-call]: ../../../../crates/memory-store/src/lib.rs#L9111-L9121
[test]: ../../../../crates/memory-store/src/lib.rs#L18744-L18785

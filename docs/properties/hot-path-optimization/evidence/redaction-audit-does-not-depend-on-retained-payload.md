# redaction-audit-does-not-depend-on-retained-payload

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

Content preparation returns a clone while retaining the redacted String in its
scan. The audit writer needs metadata, suggesting allocation work can be avoided.
This is a performance hypothesis, not a claim that retained text is unredacted.

## Evidence trail

- [memory-store/lib.rs:2221-2242][retain] clones Content output and retains the
  Redaction in the scan. Identity branches return the input value instead.
- [2133-2143][json] already records JSON-observed scans with empty retained text
  and preserved detections, field identity, action, and owners.
- [2356-2424][audit] persists detector revision/digest, finding count, owner
  copies, labels, and actions. It does not consume field.redaction.text.
- [2245-2271][execute] co-commits applied effects and audit under a fenced write.
  Replay skips a new audit append at this boundary.

## Failure scenario

An ownership transfer moves the entire Redaction out and loses its detections,
or reconstructs audit data from already-redacted text and loses findings.
Another refactor writes audit separately from effects, leaving one without the
other after refusal. Neither failure is detected by checking output text alone.

## Timing windows and dependencies

Metadata must survive the transfer until the applied write commits. Multiple
fields can share owners, and existing scan links preserve relationships that a
flat count cannot check. There is no String object-identity contract to retain.
An allocation benefit needs separate measurement, not stronger privacy wording.

## What a test must construct

Hold prepared detections, policies, field IDs, owner links, and detector build
provenance equal, then compare normalized persisted audit projections and their
effect associations regardless of retaining or discarding redacted text. Exercise
clean, detected, JSON-observed, replay, and failed writes. This is observable
dependency equivalence, not a safety requirement on an internal struct shape.
Use a unique long synthetic secret sentinel and search substituted output plus
payload-free audit columns for that exact sentinel. Exempt legitimate preserved
identity values; arbitrary short substring absence is not a privacy oracle.
Sentinel absence checks the existing privacy contract, not allocation removal.
Existing checks are unaudited, and no new parity or allocation run occurs here.

## Investigation log

### Q: Does dropping retained redacted text establish a security improvement?

- Sources examined: [Content ownership][retain], [JSON scans][json], and
  [audit persistence][audit].
- Findings: The retained payload is already redacted and persistence uses its
  metadata. Moving text can remove redundant allocation without changing policy.
- Missing evidence: A concrete candidate and allocation measurement are not
  supplied; functional parity has not been exercised.
- Conclusion: Security-fix wording is unsupported. The review's concern about
  mandating an internal shape is narrowed to observable dependency equivalence.
  M5 allocation acceptance still needs separate owner-approved measurement;
  neither receipt parity nor sentinel absence proves allocation savings.

[retain]: ../../../../crates/memory-store/src/lib.rs#L2221-L2242
[json]: ../../../../crates/memory-store/src/lib.rs#L2133-L2143
[audit]: ../../../../crates/memory-store/src/lib.rs#L2356-L2424
[execute]: ../../../../crates/memory-store/src/lib.rs#L2245-L2271

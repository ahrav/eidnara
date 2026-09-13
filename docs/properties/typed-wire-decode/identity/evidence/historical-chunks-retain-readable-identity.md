# historical-chunks-retain-readable-identity

## Discovery trigger

Plan R6 requires old `raw_chunk_messages` to decode and predicts a todo-item
fingerprint change. The claim combines recovery and item selection.
Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan R6, latency-audit W5, and issue 350.
Lenses: state/persistence, failure recovery, versioning, resource boundaries.

## Evidence trail

1. `crates/daemon/src/historian_chunk.rs:665-675` serializes in-range
   nonsynthetic ingress messages as a raw message array.
2. `crates/daemon/src/historian.rs:484` passes that string to store publication.
3. `crates/memory-store/src/lib.rs:11298-11321` scans/prepares raw history;
   lines 12012-12020 decompress stored bytes into `raw_messages_json`.
4. `crates/daemon/src/lib.rs:16443-16461`, read with `git show HEAD`, decodes
   rows as `IngressMessages`, skips decode errors, keeps in-range ordinals,
   and keeps the first message at each ordinal.
5. `crates/daemon/src/historian_chunk.rs:418-430` builds snapshot items only
   from nonsynthetic, nonsystem blocks within the selected ordinal range.
   Each item gets `block.bytes.len()`, not rendered transcript length.
6. `crates/daemon/src/historian.rs:140-157` joins `id:kind:byte_len` items.
   It is a literal fingerprint string, not a cryptographic content hash.
7. Lines 326-334 reject mismatches. Same-length content drift is outside this
   string's detection ability; selected-range identities cover another domain.

## Failure scenario

A new decoder rejects a valid stored row, so expansion quietly loses messages.
Or reserialization changes a selected plugin block length across an upgrade,
making an in-flight durable fingerprint fail. Merely asserting no error on
the recovery API misses its skip-on-error behavior.

The oracle therefore lists recovered IDs/ordinals/known fields independently
and compares the actual production item set before its fingerprint string.
Redacted historical content must be compared with the stored redacted baseline,
not the secret-bearing pre-storage request.

## Timing windows and dependencies

History is optional in ordinary operation, hence `always-or-unreached`.
The companion campaign must supply history and cannot claim an optional skip.
Reachability is default-production through durable expansion and historian
assembly. A cross-binary in-flight recovery run remains missing evidence.

## What a test must construct

- An old-release serialized ingress array with tool/default/optional forms.
- Valid ordinal bounds and independently named expected recovered messages.
- Real nonsynthetic tool call/result blocks in a selected chunk range.
- A synthetic todo pair beside them, verifying its exclusion from snapshot
  items rather than forcing it into a production claim.
- Exact UTF-8 byte lengths and the ordered literal fingerprint string.
- Record `typed_wire_identity_old_rows` and
  `typed_wire_identity_synthetic_snapshot_inputs` from supplied inputs.

## Investigation log

### Q: Does production change one todo chunk item's length by 25 bytes?

- Sources examined: `historian_chunk.rs:418-430,665-675`, synthetic constructors,
  and `compute_chunk_fingerprint`.
- Findings: production snapshot selection excludes synthetic blocks. A
  manually supplied synthetic item can change, but it is test-only. When the
  sole change is removing the false member, its actual block delta is 26 bytes.
- Missing evidence: any alternate production path including the synthetic pair.
- Conclusion: needs human input to correct or classify the plan's example;
  the inspected production path has no such changed item.

### Q: Has old-release recovery been exercised here?

- Sources examined: W5 record and the source recovery/format tests.
- Findings: W5 explicitly lacks an in-flight binary-upgrade witness.
- Missing evidence: replacement decoder, frozen old rows, and upgrade execution.
- Conclusion: unresolved. `/testing:test-strategy` owns readability and item
  oracles; a new crash claim would route to simulation/crash-testing separately.

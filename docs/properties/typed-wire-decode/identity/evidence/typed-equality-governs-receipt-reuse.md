# typed-equality-governs-receipt-reuse

## Discovery trigger

The plan removes `original` from equality and from `block_identity_digest`.
It says ingress and constructed typed-equal blocks may then reuse receipts.
Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan equality/receipts section, latency-audit B1, issue 426.
Lenses: concurrency, replay, unproven assumptions, wildcard.

## Evidence trail

1. `crates/memory-store/src/lib.rs:232-241` derives equality over kind,
   provider extras, and original at HEAD. The proposed field set is smaller.
2. `crates/daemon/src/wire.rs:870-879` reuses a receipt only after equality.
3. Lines 882-903 hash an explicit equality tuple. `IdentityFormatter`
   maps both floating signed zeros to positive zero for equality indexing.
4. `crates/daemon/src/transform.rs:167-191` chooses a positional candidate
   first. Only an absent position enables the digest-indexed fallback.
   `or_insert` keeps the first candidate for each index/digest.
5. Lines 195-200 reuse the selected receipt or compute a fresh one.
   Lines 165-166 compute canonical message bytes independently.
6. Lines 13813-13830 assert integer zero differs from floating zero, floating
   signed zeros compare equal, and their serialized strings differ.
7. Lines 13889-13892 assert reused receipts can differ from fresh ones while
   canonical message bytes stay identical.
8. Lines 13904-13938 make first-match selection independently observable and
   assert an unequal positional candidate prevents fallback to another index.

## Failure scenario

The new digest still includes representation provenance, so equal typed
blocks land in different buckets. Alternatively, digest equality bypasses the
structural check, or positional mismatch incorrectly searches other positions.
Any of these changes receipt attribution without necessarily changing payload.

The property requires equality implies equal digest, not the converse.
The oracle spells out candidate order and keeps the equality recheck.
It does not claim collision-free SHA-256 or universal byte equality from E.

## Timing windows and dependencies

The index is request-local and lazily built. Force the missing-position path.
Reachability is default-production: `from_message_reusing` uses the same
algorithm for ordinary served blocks, reductions, and rebuilt messages.
Signed-zero inputs are explicit compatibility probes, not a claim that the
plugin's JSON.stringify emits negative zero.

## What a test must construct

- Equal kind/extras from ingress decode and typed constructors.
- Different provider extras, plus accepted unknown-envelope normalization.
- Multiple equal candidates with independently distinguishable receipts.
- A missing position and a present unequal position, tested separately.
- Floating positive/negative zero in both orders and integer zero as a control.
- Equal payload bytes with and without receipt reuse for the same message.
- Record `typed_wire_identity_receipt_candidates` before receipt assertions.

## Investigation log

### Q: Does `IdentityFormatter` make signed-zero served bytes agree?

- Sources examined: `wire.rs:889-894`, `served_json.rs:121-141`, and the
  explicit signed-zero assertions in `transform.rs:13825-13892`.
- Findings: normalization affects only equality hashing. Served spans retain
  the sign, and reuse intentionally returns the selected projection receipt.
- Missing evidence: a replacement test retaining these semantics after the
  original field is removed.
- Conclusion: resolved as implementation fact; parent wording needs human
  input to avoid promising equality of fresh and reused byte fingerprints.

### Q: Can a digest match replace equality verification?

- Sources examined: the equality recheck and the first-candidate index.
- Findings: the code requires equality; a digest is only candidate selection.
- Missing evidence: none for the contract; new field-set coverage is missing.
- Conclusion: resolved. `/testing:invariant-test-review` audits the existing
  test; `/testing:test-strategy` designs the prospective replacement oracle.

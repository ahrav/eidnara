# fusion-selection-digest-tracks-identity-tuple

## Discovery trigger

The RP2.7 lifecycle contract makes a preparation digest change whenever
context revision, representation, or selected spans change, and makes the
idempotency key length-delimit every variable-width component. The selection
digest is the fused-order witness that both bind.

## Evidence trail

- `crates/kernel/src/envelope.rs` `operation_identity` hashes a domain tag,
  then each component's length before its bytes, and documents why: without a
  length prefix two field splits share one preimage.
- `crates/retrieval/src/fusion/` `Derivation` writes a domain tag, a
  count before each sequence, and a length before each component.
  `SelectionDigest::derive` hashes the ordered occurrence identifiers;
  `PreparationDigest::derive` hashes the context revision, representation,
  each selected span with its occurrence and normalized range, and the
  selection digest.
- Span normalization follows the kernel: `None` is the whole buffer, encoded
  as a single zero byte; a range is a one byte followed by two big-endian
  `u64` bounds.

## Failure scenario

A digest that concatenated components without lengths would let context
revision `ab` with representation `c` equal revision `a` with representation
`bc`, so a preparation made against one context could be confirmed against
another.

## Timing windows and dependencies

None. Derivation is a pure function.

## What a test must construct

- One selection and every reordering, extension, and truncation of it.
- One preparation input set with each component varied alone, including a
  span bound, whole-buffer versus range spelling, span order, and span count.
- The `("ab", "c")` versus `("a", "bc")` split pair.

## Investigation log

### Q: Does the preparation digest bind the accounting profile?

- Sources examined: RP2.7 #630 lifecycle contract; RP2.8 #629 application
  contract.
- Findings: RP2.7 binds the accounting profile in the idempotency key; RP2.8
  lists it among preparation digest inputs.
- Missing evidence: an owner decision reconciling the two.
- Conclusion: needs human input; this change binds the four components RP2.7
  names for the preparation digest.

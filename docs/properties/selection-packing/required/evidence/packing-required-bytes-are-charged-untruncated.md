# packing-required-bytes-are-charged-untruncated

## Discovery trigger

RP2.8 acceptance row AC3 requires required-only cost at the limit to succeed
and at limit plus one to fail with required bytes unchanged, and Q10 forbids
the 64 KiB per-line cut on the packing path.

## Evidence trail

- `crates/retrieval/src/packing/required.rs` `reserve_required` charges every
  admitted item through the injected cost function and refuses a sum above the
  limit; it never slices bytes.
- `crates/retrieval/src/packing/mod.rs` `load_payload` returns the whole
  payload row and refuses a digest or length mismatch.
- `crates/daemon/tests/packing_required.rs` uses a one-token-per-byte
  estimator so the limit sits on a byte boundary, then compares materialized
  bytes to the persisted payload and item costs to `charged`.
- `crates/daemon/src/packing/mod.rs` test shows the legacy memory line rendering a
  64 KiB + 7 payload shorter than its content while the estimator charges the
  tail.

## Failure scenario

A cut payload changes the meaning of required context without a signal; an
uncharged byte lets the serialized body exceed the provider limit.

## Timing windows and dependencies

None.

## What a test must construct

- Two payloads whose lengths sum to the limit, then a limit one below.
- A payload longer than 64 KiB.

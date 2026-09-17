# packing-required-bytes-are-charged-untruncated

## Discovery trigger

RP2.8 acceptance row AC3 requires required-only cost at the limit to succeed
and at limit plus one to fail with required bytes unchanged, and Q10 forbids
the 64 KiB per-line cut on the packing path.

## Evidence trail

- `crates/retrieval/src/packing/required.rs` `reserve_required` charges every
  admitted item through the injected cost function and refuses a sum above the
  limit; it never slices bytes.
- `crates/retrieval/src/packing/mod.rs` `fetch_payload` returns the whole
  payload row, refusing in SQL a row whose length disagrees with the reference
  or exceeds the kernel payload bound; `PayloadRef::verify` refuses a digest or
  length mismatch afterwards. `crates/retrieval/tests/packing_identity.rs`
  `payload_fetch_is_length_guarded_in_sql_and_verified_by_digest_afterwards`
  exercises both.
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

## Investigation log

### Q: Does the estimator see the bytes the caller will receive?

- Sources examined: `crates/daemon/src/packing.rs` `prepare_required`, where
  `reserve_required` is given `bytes.iter().map(Vec::as_slice)` and the same
  `bytes` are moved into `MaterializedRequired`;
  `crates/retrieval/src/packing/required.rs` `reserve_required`, which refuses
  a length that disagrees with the row's `byte_length`.
- Findings: one `Vec<u8>` per item is charged and then returned; no copy or
  cut sits between the two uses.
- Missing evidence: none.
- Conclusion: resolved with answer - the charged bytes are the returned bytes.

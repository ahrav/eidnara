# prepared-output-diagnostics-preserved

## Discovery trigger

Plan R4 preserves the existing response path. The output-verification lane
finds variant-specific errors and first-crossing length diagnostics that a
successful-byte comparison would miss. This is preservation, not a new
short-write, direct-output, or transport contract.

## Evidence trail

- [Measured writes][write] map Json destination I/O errors to `Write`, keep
  non-I/O serde errors as `Serialize`, and verify actual versus measured length.
- [Transform writing][transform] uses `Write` for punctuation/segments but
  `Serialize` for serde-wrapped key or envelope-value failures.
- [Counting][count] records the first over-cap addition. `finish_count` returns
  that `BodyTooLarge { len, max }` or `LengthOverflow`; it does not finish the
  body to discover the eventual length. Known totals use a separate helper.
- [Settlement][settle] measures, checks cancellation, reserves, checks again,
  then writes. Measurement/write errors map to `encode_failed`; the two cuts
  use `request_cancelled` with different messages; denial is `output_unavailable`.
- [Public tests][public] cover cap, overflow, partial failure and mismatch.
  [Private tests][private] cover reservation ordering, cancellation and denial.
  All are unaudited and unrun here.

Anchors were verified with `git show HEAD:<path>` at
`2e4433e6b511ae74944df8a9669c428e73915d29`, separate from plan baseline
`4980f8af`. The lane's candidate “partial” exercise label is not retained:
inspection alone establishes no exercise at this revision.

## Failure scenario

Changing count/write partitioning can change the first oversize length even
when the eventual body is identical. Flattening error conversions can turn an
envelope `Serialize` error into `Write`. Reserving before a cap refusal or
writing after a cancellation cut can alter failure behavior with unchanged
successful output.

## Timing windows and dependencies

Before/after diagnostic comparison fixes the PreparedSource variant, cap,
measurement call partition, and write call partition. There is no equality
requirement across different variants or chunk partitions. Preserve the
existing count/write envelope passes; add no new message-length serialization.

Serialization of served messages precedes output reservation. Settlement
retains the prepared source through its existing await and cancellation cuts.
The optimization adds neither a callback nor a new cancellation state machine.
Wire cancellation remains best effort, not retraction after publication.

## What a test must construct

Extend existing public `measure`/`write_to` and private `settle_prepared_with`
tests. Use Json, Exact, and Transform fixtures, including actual served
segments through the existing B1 shell test. On success, compare measured
length, actual written length, and bytes. Include empty/multiple messages.

For failure, use exactly-at-cap, cap-plus-one, overflow, inconsistent reported
segment length, reservation denial, cancellation at each cut, and destination
failure after accepted bytes. Inject at punctuation/segment and serde-envelope
positions to preserve their different error classifications. Assert exact
error variants, codes, messages, and length fields, not just `is_err()`.

Count reservation and write events. Cap refusal and pre-reservation cancellation
must reserve zero times; cancellation after reservation must write zero body
bytes. Denial must return no Response. Failed writes can leave bytes in the
private destination but must return no successful Response.

Use the existing writer contract when extending positive-short-write or other
I/O fixtures. Do not import a new transport short-write matrix as acceptance
for this optimization. The public partial writer's local `terminal = None`
only records its test's success branch; it is not an observation of a ring.
T3/T4 remain separate, neither delivered nor exercised by this record.

## Investigation log

### Q: Must every source report the same error and full oversize length?

- Sources examined: error conversions, CountingWriter, and checked totals.
- Findings: error variants depend on the path; oversize counting stops at the
  first crossing. Equal eventual bodies do not imply equal diagnostics.
- Missing evidence: the missing same-variant/partition comparison cases.
- Conclusion: resolved with answer. Preserve exact behavior under identical
  partitions, not an invented uniform contract.

### Q: Do public writer tests prove transport publication safety?

- Sources examined: public partial-writer test and settlement function.
- Findings: a local terminal variable is not a ring witness.
- Missing evidence: direct-frame publication/lifetime evidence belongs to HP1.
- Conclusion: resolved with answer. Route S7 to `/testing:test-strategy`, tests
  to `/testing:invariant-test-review`, guards to the separate production audit.

[write]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/dispatch.rs#L237-L277
[transform]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/dispatch.rs#L343-L371
[count]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/dispatch.rs#L280-L330
[settle]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L12360-L12431
[public]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/prepared_output.rs#L118-L234
[private]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L17823-L17939

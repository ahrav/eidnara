# decode-footprint-covers-both-lanes-combined-peak

System: daemon decode. HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`.
Date: 2026-09-13. [Source register](../source-register.md) defines P and B.
This is a claim under test. No new allocation measurement is performed.

## Discovery trigger

P:L88 proposes string copies of one, keeps node copies at two, and requires
raising the string coefficient to the smallest passing value if needed.
P:L51 requires footprint coverage on both lanes. End-state ownership alone
does not determine the simultaneous allocation peak.

## Evidence trail

1. `crates/memory-store/src/lib.rs:126-143,250-264` builds an original Value,
   clones it, deserializes typed fields, and retains the original. This is
   HEAD behavior, not the tree-free target.
2. `crates/daemon/src/metered_decode.rs:33-64` sets node copies to two,
   node-growth slack to two, string copies to three, envelope headroom to
   4096 bytes, and unescape-growth slack to two.
3. `crates/daemon/src/lib.rs:12921-12946` first runs the direct gate, then
   typed decoding, then tree fallback only for invalid decoding.
   `:8115-8123` consumes the Value into a TransformRequest.
4. `crates/daemon/src/metered_decode.rs:201-207` resets needed counters but
   retains charges. `:242-244` reserves unescape scratch from input bytes;
   `:329-348` handles refusal and trailing-byte errors.
5. `crates/daemon/tests/parse_charge_covers_typed_decode.rs:110-123` correctly
   starts its native-array peak before `from_slice::<Value>` and keeps it
   through `from_value`. It is not a second-stage-only measurement.
6. The text cases at `:178-199` run only direct decoding. The allocation
   counter at `:28-29` explicitly excludes allocator rounding.
7. `crates/daemon/src/lib.rs:20281-20315` contains
   `decode_footprint_counts_values_and_retained_string_copies`; its assertion
   at `:20294-20298` requires at least three times the text length. This needs
   explicit disposition under KTD4, not silent removal from the test inventory.

Default-production reachability follows the real handler's lane dispatch,
not a test feature. Rare corpus conditions still need explicit construction.
All existing checks remain unaudited.

## Failure scenario

After envelope-tree removal, a large escaped string can coexist with serde
scratch and a typed buffer. Tree conversion can retain containers while
constructing typed shells. A failed direct attempt can allocate a large
prefix before the tree path starts. Taking only final retained size or
resetting the counter at conversion hides these costs.

Competing explanation: an apparent peak violation is fixture allocation or
allocator noise, rather than undercharging. A continuous attributed trace
with construction outside the interval distinguishes the two.

## Timing windows and dependencies

Use one baseline before the first parse. Preserve inherited live allocations
when entering `from_value`; do not add independent peak deltas. Include the
error object and cleanup endpoint on failure. On fallback preserve the same
trace across meter restart and the second parse.

`F_k_max` must include the upfront raw escaped-string reservation, not merely
the last needed value after restart. The static footprint walker counts a
visited prefix on malformed input (`metered_decode.rs:351-359`); retain that
distinction instead of claiming every failure has a full-body footprint.

## What a test must construct

- Plain and escaped text, including a long escaped object key.
- Dense native arrays around vector-growth boundaries and nested Values.
- Both actual tree stages, not a tree built before measurement starts.
- Duplicate envelope keys and late type errors after a large prefix.
- Trailing garbage, malformed escapes, and refused allocations.
- A direct meter trace and a production fallback trace alongside the isolated
  unmetered `from_slice`/`from_value` calibration.
- Check `P_decode <= F_k_max` for the selected coefficient.

KTD4's selection procedure is separate: keep two node copies, start string
copies at one, and raise to the smallest passing positive integer over the
declared matrix. Retain failed smaller-value observations. This accepted
decision does not become an additional runtime predicate.

## Investigation log

### Q: Is one string copy established, and is it globally minimal?

- Sources examined: P:L88, P:L250-L267; meter constants; parse-peak tests.
- Findings: One is a proposed starting value. The reported derive-only mirror
  does not include a final candidate or all failure/fallback cases.
- Missing evidence: Final artifacts and a complete per-lane peak matrix.
- Conclusion: unresolved, needs measurement. Minimal means the smallest
  passing positive integer over the declared corpus/build envelope, not a
  universal proof over all inputs and allocators.

### Q: Does a requested-layout counter prove exact RSS?

- Sources examined: `crates/daemon/tests/parse_charge_covers_typed_decode.rs:28-29`.
- Findings: The counter is a floor and excludes allocator rounding. The host
  contract at `crates/host-runtime/src/config.rs:65-72` bounds named logical
  payloads rather than exact RSS.
- Missing evidence: Final candidate layout-peak and logical-pool observations.
- Conclusion: resolved on semantics. Preserve that budget model; no new exact
  RSS policy or allocator-headroom design is required here.

### Q: Was the hard-coded-three test absent from the inventory?

- Sources examined: The pre-disposition existing-checks row for `lib.rs:20281`
  and the HEAD assertion at `:20294-20298`.
- Findings: The row already exists. R1 and this evidence trail omitted the
  direct link, so the reported inventory omission is only partly confirmed.
- Missing evidence: Candidate disposition of the three-copy assertion.
- Conclusion: resolved for discovery. Link restored; adequacy stays unaudited.
  See independent finding 4 in [portfolio-evaluation.md](../portfolio-evaluation.md).

## Typed-wire U1 execution, 2026-09-13

Branch `perf/typed-wire-u1-owned-decode`. `RETAINED_STRING_COPIES` is 1 and
`RETAINED_NODE_COPIES` stays 2. `cargo test -p daemon --locked --features
test-support --test parse_charge_covers_typed_decode` passes ten tests. Peaks
are requested layout bytes from the test's counting allocator, measured from
before the parse through the returned value or the failure cleanup.

| Case | Lane | Result |
| --- | --- | --- |
| 4 MiB plain text, 4 MiB escaped, 64 KiB escaped | direct and tree, upfront scratch charge taken | `peak <= needed` (`parse_charge_covers_text_heavy_peaks_on_both_lanes`) |
| dense native values, 65,536 / 65,537 / 262,144 elements | tree then direct, metered ignored field | `peak <= charge` (`parse_charge_covers_dense_native_typed_decode_peak`) |
| 4 MiB text then a duplicate `mid` in the last message, plain and escaped | walk, typed failure, restart, tree, conversion | `peak <= max(footprint, typed needed, tree needed)` (`parse_charge_covers_a_failed_typed_prefix_and_its_tree_fallback`) |
| 64 and 4,096 tool-call blocks with nested input and extras | direct | `peak <= needed`: 216,033 <= 239,675 at 64 blocks (`parse_charge_covers_payload_heavy_direct_decode_peak_and_pins_the_tree_lane_gap`) |
| same bodies | tree | exceeds: 309,426 against 239,675 at 64 blocks; 19,645,338 against 14,930,339 at 4,096 blocks; the same test pins the excess at no more than one third of the charge (`TREE_LANE_CONTAINER_SLACK_DENOMINATOR`) and fails if the gap closes, so the pin can be removed |

The tree-lane excess on payload-heavy bodies is container storage: every small
object becomes a `BTreeMap` leaf node (eleven slots) before conversion, and the
per-value node charge does not cover a one-entry map. The bodies carry 3,643 and
243,619 string bytes, so no string coefficient closes the gap (it would need
about twenty copies). The gap predates this change: at three copies the charge
was 245,000 and 15,400,000 for the same peaks. The direct lane, which is the
production lane for these bodies, fits. Coefficient selection therefore stops at
one: raising it would not make the tree lane fit and would only narrow the
admitted set.

Open question retained: the tree lane's per-object charge for map-dense bodies
needs an owner decision outside KTD4, which fixes node copies at two.

Text-heavy ceiling witnesses at an 8 MiB pool
(`text_heavy_admission_ceiling_witnesses`): plain text of 8 MiB minus 64 KiB is
admitted and 8 MiB plus 64 KiB refused as too large; escaped text of one third
of the pool minus 64 KiB is admitted and plus 64 KiB refused. Under the three-copy
charge the same pool admitted one third and one fifth of the pool. At the
default pool the 32 MiB transform length cap binds first.

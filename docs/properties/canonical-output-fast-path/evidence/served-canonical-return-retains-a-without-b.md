# served-canonical-return-retains-a-without-b

## Discovery trigger

Plan R1 requires returning A without B, including ordered escaped keys. The
initial independent findings reject allocation slope as proof: a fixed B
allocation occurs at both input sizes and disappears from their difference.

## Evidence trail

- [Encode][encode] grows A, then always allocates B with requested capacity
  `A.len()` and copies the whole output at HEAD. This is not the fast path.
- [The existing allocator][allocator] counts alloc/realloc events only.
  [Its assertion][slope] bounds `(large_events - small_events) / 64` for 1/65
  blocks. It cannot attribute fixed B, bytes, lifetimes, or copies. Unaudited.
- [The integration test-support entry][entry] reaches only the canonicalizer,
  not the private full constructor.
- [The full constructor][constructor] holds the returned Vec during block
  receipt construction and hashing, then converts it to an Arc slice.
  Returning A can retain slack throughout all those operations.
- Allocator search finds only [parse-charge PeakAlloc][peak] and the event
  fixture in daemon. Neither provides an in-crate unit-binary allocator at HEAD.

All numbered anchors were verified with `git show HEAD:<path>` at
`2e4433e6b511ae74944df8a9669c428e73915d29`, not the plan's `4980f8af`
baseline. No allocation or timing result is claimed.

## Failure scenario

An implementation can clone A, allocate/copy/discard B, or shrink A and still
pass byte checks. A B-site counter alone misses replacement scratch; pointer
equality alone misses work discarded before returning the original pointer.
The always-copy baseline must fail a candidate no-B oracle as a negative control.

## Timing windows and dependencies

The ownership interval starts after serialization and ends at encoder return,
before Arc conversion. Capture A's pointer, allocation lifetime, length, and
capacity without allocation. Check no intervening free/realloc/shrink and
exact identity at return. Allocation observation covers the entire encode
so hidden work before the captured boundary cannot escape detection.

Use a large scalar with small span tables and short keys, built outside the
interval. Attribute every output-sized allocation to A's growth chain; no
other output-sized scratch is allowed. Record actual metadata/key sizes. For
small outputs the span tables and sort scratch themselves reach N, so the
retained harness identifies A by the root allocation its growth chain starts
from (the capacity `Vec<u8>` gives a one-byte write) and requires no second
buffer with that root to reach N, rather than counting every allocation of at
least N bytes.
Record logical reorder output, including reconstructed braces and commas:
a full reorder materializes N bytes. This is not a count of physical copies
caused by allocator growth or Arc conversion.

## What a test must construct

Extend the existing allocation fixture and private observations, not public
production APIs. Include canonical misses with unescaped and escaped keys;
establish canonicality independently from emitted key sequences. Cache hits
are not saving witnesses. Keep reference encoding, assertions, formatting,
and S3's test-only comparison outside the measurement interval.

The global allocator needs preinitialized, nonallocating, thread-owned
recording enabled only around the tested call. A mutex between tests cannot
exclude libtest/harness allocations on other threads. A fixed-capacity ledger
must report overflow rather than silently lose events. Use both explicit
isolated invocations below after extending the existing test; these commands
were not run in this documentation pass:

```sh
cargo +1.98 test -p daemon --all-features --locked \
  --test served_json_passthrough_allocations \
  passthrough_shell_canonicalization_allocates_independently_of_key_count \
  -- --exact --test-threads=1
cargo +1.98 nextest run --profile ci -p daemon --all-features --locked \
  --test served_json_passthrough_allocations --test-threads 1 \
  -E 'test(=passthrough_shell_canonicalization_allocates_independently_of_key_count)'
```

Confirm exactly one test executes in each process. Serial invocation alone
does not replace owner-thread filtering. Keep the same harness revision for
baseline and candidate binaries, without an old in-tree production variant.

Full-constructor peak needs a separate observer. An in-crate (`--lib`) test
observer is infeasible: `crates/daemon/src/lib.rs` declares
`#![forbid(unsafe_code)]`, so the crate cannot declare a `GlobalAlloc`. The
retained harness installs `RecordingAlloc` in the integration test binary and
observes the no-projection constructor arm through the `test-support` entry
`daemon::transform::served_message_for_test`, in
`full_constructor_observation_covers_receipts_hashing_and_arc_conversion`
(`crates/daemon/tests/served_json_passthrough_allocations.rs`). Run it with the
same exact one-test filter and owner-thread gating as the invocations above.
The entry compiles only under `test-support`; no public constructor
wrapper/API, production hook, harness framework, dependency, or disabling of
the cfg(test) fresh differential is added.

Measure peak live bytes and A capacity/length through fingerprints, hashing,
and Arc conversion. B1 preserves final Arc payload bytes/length; transient A
slack ends at conversion, but unchanged retained accounting is not global RSS.
Peak remains unknown, with no universal residency win. Plan U0/U4 retains all
metrics and ten AB/BA process pairs; isolated canonicalizer/full-constructor
timing cells remain absent. A real host driver is needed only to claim
user-visible latency, not to establish this local resource invariant.

## Investigation log

### Q: Does removing one allocation from a slope prove R1?

- Sources examined: the unconditional B site and event-only allocator.
- Findings: fixed allocations cancel; hidden replacement storage also escapes
  a single site counter. Absolute attribution is necessary.
- Missing evidence: owner-thread recording, candidate runs, negative control.
- Conclusion: resolved with answer. Hand implementation to `/testing:test-strategy`
  and existing-check adequacy to `/testing:invariant-test-review`.

### Q: Is full-constructor peak lower?

- Sources examined: constructor order and plan U0/U4.
- Findings: A slack survives the receipt loop and hashing before Arc conversion.
  The private in-crate seam resolves access; U0 still must build its observer.
- Missing evidence: measured full-constructor peak and population frequencies.
- Conclusion: unresolved, needs measurement and investigation of any increased
  peak before landing. Do not invent a `2 * N` bound or claim a speedup.

[encode]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L121-L142
[allocator]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/served_json_passthrough_allocations.rs#L10-L32
[slope]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/served_json_passthrough_allocations.rs#L58-L86
[entry]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L111-L119
[constructor]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L164-L224
[peak]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/parse_charge_covers_typed_decode.rs#L25-L87

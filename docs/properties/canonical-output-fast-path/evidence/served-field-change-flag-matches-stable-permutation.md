# served-field-change-flag-matches-stable-permutation

## Discovery trigger

Plan KTD1 changes `sort_fields` from an ordering operation to an ordering
operation that also authorizes ownership transfer. The flag needs its own
oracle: unchanged output bytes cannot detect an unnecessary B allocation.
This refines B1 rather than creating another byte-compatibility guarantee.

## Evidence trail

- [Recording][record] appends a field start after any leading comma and records
  key end and value end. Original field starts are unique and increasing.
- [Sorting][sort] returns early for fewer than two fields. Unescaped keys
  compare between quotes; escaped keys decode as `String` and use stable
  cached-key sorting. HEAD has no changed-order result.
- [Encoding][encode] invokes this helper for every recorded object. The
  production WireMessage facade reaches it without a special configuration.
- [Existing tests][tests] compare prefix and escaped-key outputs, but do not
  assert a permutation flag or duplicate-key stability. Status: unaudited.

These numbered links were checked with `git show HEAD:<path>` at
`2e4433e6b511ae74944df8a9669c428e73915d29`. The settled plan's baseline
`4980f8af3bb90d58b19b80a38a227fb6363a8b33` is separate. No test ran here.

## Failure scenario

A raw escaped-spelling comparison can treat a decoded inversion as ordered.
That false negative can return noncanonical A. A flag that marks already
ordered escaped keys as changed can preserve B1 while violating plan R1.
An unstable equal-key sort can exchange distinct values even though all keys
still appear nondecreasing.

## Timing windows and dependencies

There is no scheduling fault. Save complete field descriptors before sorting.
The identity predicate depends on unique original starts: a strictly
increasing permutation of those starts is exactly the identity permutation.
This predicate verifies the result; it must not be its own expected oracle.
Empty and singleton cases retain the early guard before decoding.

## What a test must construct

1. Obtain A and F from actual formatter recording in the existing module.
2. Decode emitted keys independently. Stable-sort source indices by
   `(decoded key, original index)` to obtain expected permutation P.
3. Assert exact descriptor equality to `F[P]`, preserving ranges and key ends.
   Assert `changed == (P != identity)` and the increasing-start equivalence.
4. Exhaust all six permutations of three distinct keys. Include empty and
   singleton objects and both ordered and disordered escaped-key objects.
5. Include empty keys, `a`/`a b`, quotes, backslashes, controls, and Unicode.
   Use private generic sources for duplicate keys with distinct value tags,
   both already grouped and displaced by another key.

Production-reachable shapes establish `default-production` for this record;
duplicate-emitting sources remain test-only witnesses. Do not collapse them
through `Value`. Its ordering reference is conditional on map features;
decoded permutations and literals do not require `preserve_order` off.
Reachability follows the nonvacuous core obligation: production objects require
the permutation check even without duplicate probes. By contrast, S4's error
clause needs private failing inputs to be exercised rather than pass vacuously.

## Investigation log

### Q: Does a byte oracle also prove the changed flag?

- Sources examined: the sorter, copier call site, and existing key fixtures.
- Findings: false positives can still produce correct bytes through B.
  S5's absolute allocation oracle distinguishes that failure from success.
- Missing evidence: a changed result and private observation on a candidate.
- Conclusion: resolved with answer. Keep distinct flag and allocation checks;
  no runtime exercise is established.

### Q: Which owner implements and audits the checks?

- Sources examined: plan U1 and the existing module tests.
- Findings: the private module already accepts controlled generic sources.
- Missing evidence: implemented assertions and discriminating runs.
- Conclusion: unresolved, needs `/testing:test-strategy`; existing tests go to
  `/testing:invariant-test-review`, and guards to the separate guard audit.

[record]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L42-L81
[sort]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L144-L164
[encode]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L111-L142
[tests]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L195-L252

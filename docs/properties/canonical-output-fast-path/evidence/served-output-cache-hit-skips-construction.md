# served-output-cache-hit-skips-construction

## Discovery trigger

The ownership lane identifies a distinct bypass property. B1 byte equality
cannot detect reconstructing the same artifacts on a cache hit. Warm-cache
measurements also cannot prove the selected canonical-miss saving.

## Evidence trail

- [ServedMessage][owner] stores separate Arcs for the message, canonical
  bytes, output identity, and block fingerprints; cloning preserves owners.
- [The production caller][caller] passes its output cache; compaction is
  enabled by [default configuration][config].
- [Synthetic lookup][lookup] requires a clean matching entry and returns a
  cloned served value on a positive hit without invoking the build closure.
- [Live-tail lookup][tail] also reuses a cached served result. Its `Some(None)`
  omission increments hits too, but is not a positive served-message hit.
  The synthetic helper flattens that omission into a miss at lookup.
- [The steady replay test][test] asserts zero serialized and four reused
  items plus byte equality. These counters are not independent call spies.
- [The test differential][differential] deliberately builds fresh output
  after cached output; whole-pass encoder counts would include reference work.

Numbered links were verified using `git show HEAD:<path>` at
`2e4433e6b511ae74944df8a9669c428e73915d29`. Plan baseline `4980f8af` is
separate. All inspected checks are unaudited and unrun here.

## Failure scenario

A hit path can rebuild identical bytes and still report reuse. That restores
serialization, hashing, allocation, and copying while B1 remains green.
Conversely, counting the test's intentional fresh differential as hit work
can falsely diagnose the correct path as reconstructing.

## Timing windows and dependencies

Observe one selected positive hit, not the entire transform including setup
and references. Establish session, revert epoch, key, identity, clean status,
and a retained positive entry before lookup. Use a cold miss as a positive
control for construction outside that interval.
Require the unflattened result to contain `Some(Some(served))`; the existing
hit counters alone cannot establish that a warm positive item was selected.

The property uses `always-or-unreached`: hits are optional on any one request.
C2 separately requires the campaign to construct eligible positive hits.
This is not a whole-request allocation-free claim; snapshots and response
assembly still do work. It adds no concurrency or lifetime protocol.

## What a test must construct

Extend the existing private cache fixture for synthetic and live-tail callers.
After a miss inserts an entry, invoke the matching clean path and independently
observe zero constructor and encoder calls attributable to that item.
Compare Arc identity for the returned message, canonical bytes, output identity,
and fingerprint array against the cached owners.

Keep fresh-output differential work outside the observation interval without
disabling it. Omitted entries, dirty entries, changed identities, and changed
revert epochs are controls, not positive hit witnesses. Existing invalidation
tests can supply those states without new cache abstractions or timing fields.

For broad ownership/bytes across eviction and prepared replay, reuse B1 rather
than extending this narrow bypass record. Cache accounting release need not
immediately free allocations retained by other Arc owners.

## Investigation log

### Q: Do reused-item statistics prove no constructor call occurred?

- Sources examined: both hit branches, replay test, and fresh differential.
- Findings: source shows a bypass, but reported counters and equal bytes do
  not independently observe construction. Differential work needs scoping.
- Missing evidence: private call observations and pointer assertions.
- Conclusion: unresolved, needs `/testing:test-strategy`; existing replay and
  invalidation tests go to `/testing:invariant-test-review`.

### Q: Is prepared-result replay the same hit?

- Sources examined: the cache model and completed-page replay in C2 evidence.
- Findings: one returns ServedMessage artifacts; the other clones PreparedOutput.
- Missing evidence: no new production mechanism is needed.
- Conclusion: resolved with answer. Keep separate campaign witnesses.

[owner]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L144-L153
[caller]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L8492-L8499
[config]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/config.rs#L116-L125
[lookup]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L10392-L10432
[tail]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L11126-L11135
[test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L28295-L28320
[differential]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L4862-L4890

# served-cache-campaign-reaches-miss-and-hit

## Discovery trigger

Plan U0 separates canonical misses from typed/edited misses and warm cache
hits. The ownership lane also distinguishes PreparedOutput replay. These
populations need independent campaign witnesses, not inferred benchmark names.

## Evidence trail

- [The production transform caller][caller] supplies the serialized-output
  cache. [Lookup][lookup] tests cleanliness and identity before a positive hit.
- [Live-tail lookup][tail] also increments hits for an omitted `Some(None)`;
  the synthetic helper flattens it into a miss. Hit counts do not prove a
  positive item, so the tail marker must inspect the retained served value.
- [Typed versus retained serialization][message] explains potential order
  differences, but original presence cannot authorize the fast path.
- [Editing content][edit] clears only the message original. [Untouched blocks][block]
  independently replay their originals, including unknown sibling fields.
- [Completed-page replay][replay] checks generation, page count, final digest,
  and scalar digest before cloning its retained PreparedOutput.
- [Existing cache tests][tests] and [cold/warm benchmarks][bench] are unaudited.
  None supplies the combined campaign assertion or a measured population mix.

Anchors were verified with `git show HEAD:<path>` at
`2e4433e6b511ae74944df8a9669c428e73915d29`, separate from plan baseline
`4980f8af`. No benchmark or scenario ran in this pass.

## Failure scenario

A warm-only campaign bypasses encode and reports no B even on an always-copy
implementation. A typed-only campaign can miss the canonical path entirely.
Treating a cached omission as a positive hit or prepared-page replay as a
ServedMessage hit hides a missing construction rather than covering it.

## Timing windows and dependencies

Cold miss precedes insertion; warm hit requires the matching clean entry.
Prepared replay requires a completed attempt with matching metadata and is
tracked separately. None of these markers asserts no construction or no B.
The joint set uses `sometimes` across the campaign, not all states at once.
These are default production situations constructed by tests, not a promise
of restart durability or a new distributed coordination protocol.

## What a test must construct

Every constant marker below is required at campaign completion. Compound rows
require all listed variants, accumulated across calls. Inspect emitted keys
independently and keep that reference work outside measured intervals.
Report every missing marker separately, including incomplete composite members,
instead of only reporting an opaque failure of the accumulated AND.

| Constant marker | Independent precondition |
| --- | --- |
| `canonical_output_fast_path_cold_canonical_miss` | No eligible output entry exists; the real retained-source facade receives independently ordered emitted keys, including a multi-key escaped object and a large-scalar/small-metadata case. |
| `canonical_output_fast_path_typed_disordered_miss` | No eligible entry exists; a fully typed shell contains an emitted decoded-key inversion. |
| `canonical_output_fast_path_edited_disordered_miss` | A one-block-edited shell is dirty or lacks an eligible entry and emits an inversion; an untouched sibling retains its own original. |
| `canonical_output_fast_path_warm_positive_synthetic` | The synthetic caller has the same session/epoch/key/identity, clean status, and a retained positive ServedMessage entry before lookup. |
| `canonical_output_fast_path_warm_positive_tail` | Before live-tail reuse, the same session/epoch/key/identity and clean status select a retained `Some(Some(served))`. Inspect the positive value independently: `Some(None)` increments hits but cannot satisfy this marker. |
| `canonical_output_fast_path_prepared_page_replay` | A completed transform-page attempt has matching generation, page count, page-complete state, final digest, and scalar digest, with a retained PreparedOutput before replay selection. |

S5 checks the actual saving on the first population. S6 independently checks
bypass and Arc identity on positive hits. B1 owns byte/hash/receipt equivalence
for all populations. S7 preserves preparation behavior of replayed sources.
Keep the existing fresh-output differential outside hit-call observation.

The 1/65-block and 100/1,000-message sizes are controlled fixture points, not
empirical proof of production representativeness. Record actual canonical,
miss, and hit frequencies before attributing transform improvements to S5.

## Investigation log

### Q: Can a benchmark name stand in for a situation marker?

- Sources examined: steady and steady-output-cache benchmark setup.
- Findings: cold/warm cache setup exists, but emitted canonicality and actual
  hit populations still need independent observations.
- Missing evidence: combined markers and measured frequencies.
- Conclusion: unresolved, needs `/testing:test-strategy` for construction and
  plan U0/U4 for measurement; do not infer a user-visible latency result.

### Q: Does clearing the message original lose untouched sibling fields?

- Sources examined: content_mut and WireBlock serialization.
- Findings: block originals are independent; untouched siblings replay them.
- Missing evidence: additional B1 mixed-shell execution at this revision.
- Conclusion: resolved with answer. The alleged defect is rejected; retain
  the witness under B1 and route existing tests to `/testing:invariant-test-review`.

[caller]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L8492-L8499
[lookup]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L10392-L10432
[tail]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L11127-L11131
[message]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L114-L162
[edit]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L203-L209
[block]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L266-L278
[replay]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L9680-L9701
[tests]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L28295-L28424
[bench]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/benches/hot_path.rs#L320-L397

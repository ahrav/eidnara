# Wildcard synthesis trace

## Method and limits

This pass follows the three supplied lenses and challenges their witness
reachability, ownership interval, and proposed observation cost. It verifies
the supplied wildcard findings against pinned source rather than presenting
assumptions as facts. It is a writer refinement pass, distinct from the now
[completed independent final evaluation](../portfolio-evaluation.md).
No runtime test or benchmark runs in either pass.

All numbered anchors were checked using `git show HEAD:<path>` at
`2e4433e6b511ae74944df8a9669c428e73915d29` on 2026-09-13. The plan uses
baseline `4980f8af`; that is not this source snapshot. Dirty source is excluded.

## Findings and dispositions

### Empty default metadata is a production witness

**Verified; refinement accepted.** Every [HarnessMeta member][meta] is skipped
at its default. [WireMessageData][message] includes meta without an enclosing
skip predicate. A typed message with default metadata therefore emits `{}`.
Empty objects are not only private generic test cases. C1 requires this
production witness, and S1 preserves the fewer-than-two-fields guard before
decoding. Singleton coverage remains required too.

### Parent sortedness does not imply child sortedness

**Verified; refinement accepted.** [WireBlockData][block] emits `kind` then
optional `provider_extras`, while [BlockKind][kind] emits its `type` tag before
variant fields such as `text`. An ordered production block parent can contain
a disordered child. S2/C1 require this witness. Do not upgrade the exact
ordered WireMessage root with disordered child to production reachability:
typed roots emit `role` before `content`, and sorted-Value retained originals
are recursively ordered. That exact root witness stays private and generic
under the pinned feature assumption.
The production block case requires child traversal, but its typed message
root is already disordered. Only the private ordered-root/child-only witness
isolates a false-negative root-only aggregate decision.

### Returning A extends its slack through more than hashing

**Verified; refinement accepted.** [Construction][constructor] receives Vec
bytes, computes block receipts, hashes the canonical bytes, then converts to
Arc. A slack remains live through the receipt loop as well as hashing. S5 and
plan measurement include that full interval. Reduced local B work does not
prove lower peak residency. The peak and its effect remain unmeasured.
B1 preserves final Arc payload length and bytes. Transient A slack ends at
Arc conversion; unchanged retained accounting does not establish global RSS.
The final evaluation flags the plan-shaped portfolio as a framing bias. The
settled user scope and this downstream ownership review resolve the disposition
without another record or transport work. Seek a design decision only if
measurements require one, through the existing U4 investigation gate.

### The identity lemma must observe real encode without duplicating it

**Verified gap; implementation recommendation refined.** [The real encoder][encode]
already holds A and tables. [The copier][copy] reconstructs compact punctuation
and copies ranges. A private test-only observation can compare unchanged
tables against A even after production skips that copy. No duplicate
serializer setup, new production hook, general span-geometry validator, or
parallel canonicalizer is needed. S3 specifies the lemma and the smallest
discriminating direct comparison, leaving test form to test-strategy.

### Exact visitation does not mandate production counters

**Refinement accepted.** The S2 invariant still requires every object exactly
once. Source verification of the real non-short-circuit loop plus literal
late-disorder witnesses can discriminate the targeted bug. Per-object
production counters would add unnecessary machinery. Private observations
remain optional if a concrete failure cannot be isolated otherwise.

### An incorrect changed flag has two different consequences

**Verified; refinement accepted.** A false negative can return disordered A
and violate B1. A false positive can still return correct B bytes but violate
plan R1. S1 needs the permutation flag oracle; S5 needs absolute allocation
attribution. No byte-only test proves the no-B requirement.

### Untouched sibling unknown fields are not lost by content_mut

**Alleged defect rejected after source verification.** [content_mut][edit]
clears the message original, not each block's original. [WireBlock serialization][retained]
independently replays that block's original. An untouched sibling therefore
retains its unknown fields even when another block is edited. This remains a
B1 mixed-shell witness, not a new defect/property. Unknown fields on the edited
shell follow existing retention semantics; do not promise universal retention.

### Canonical collation is not a new wire requirement

**Claim refined.** [sort_fields][sort] preserves decoded Rust String order.
The plan preserves that behavior; the host wire document owns framing and
cancellation, not a freshly invented canonical collation rule. Value-reference
ordering remains feature-conditional. Algorithm correctness must not require
preserve_order off, and duplicate-key tests must not collapse through Value.

### Writer failure is not direct-frame proof

**Verified; expansion rejected.** [The partial-writer test][partial] controls
a local terminal variable. It cannot observe ring publication. S7 fixes the
same PreparedSource variant, cap, measurement/write partition and preserves
path-specific diagnostics. First-crossing count lengths need not equal eventual
body lengths or other partitions. T3/T4 remain separate and unexercised here.

## Assumptions and remaining evidence

[Cargo.lock][serde-lock] pins serde_json 1.0.151 with `itoa`, `memchr`, `serde`,
`serde_core`, and `zmij`, and no `indexmap`. With the manifest, this supports
the inspected locked sorted-map assumption, not a runtime-captured feature
graph. Canonical-miss frequency, full-constructor peak, CPU gain, and latency
gain remain unmeasured. Controlled 100/1,000-message
points do not prove production representativeness. The existing direct_host
seam is not a benchmark driver; add one only for user-visible latency claims.

The [catalog](../catalog.md) and [fault map](../fault-map.md) incorporate these
dispositions without editing peer lenses or historical B1/T/W records.
All records go to test-strategy; tests and production guards have separate
audit owners. The independent final evaluation accepts discovery with no
blockers; implementation observations and measurements remain to be completed.

[meta]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L73-L90
[message]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L114-L162
[block]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L243-L248
[kind]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L326-L339
[constructor]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L164-L224
[encode]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L121-L142
[copy]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L84-L109
[edit]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L203-L209
[retained]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L266-L278
[sort]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L144-L164
[partial]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/prepared_output.rs#L186-L204
[serde-lock]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/Cargo.lock#L2828-L2839

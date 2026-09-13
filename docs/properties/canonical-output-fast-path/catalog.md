# Canonical-output fast-path properties

## Scope and provenance

This catalog refines the settled [canonical-output plan][plan]. It contains
seven narrow safety obligations and two campaign situation-coverage records.
It is not a second implementation specification. The user explicitly supplied
the settled evidence scope. No additional incidents were supplied; no evidence
interview or fictional answer of “none” is recorded.

Source inspection is pinned to `2e4433e6b511ae74944df8a9669c428e73915d29`
on 2026-09-13. Numbered source links below use immutable GitHub blobs, verified
with `git show HEAD:<path>`, rather than unrelated dirty working files.
`colgrep` supplies primary discovery leads. The plan is a local artifact whose
source baseline is `4980f8af3bb90d58b19b80a38a227fb6363a8b33`, not this HEAD.
Its headings establish obligations, not implementation or execution evidence.
Historical catalog source anchors can drift; fresh links here do not rewrite
that evidence. For example, the frozen-corpus test is at HEAD line 13760,
not the plan's baseline line 13757.

The main agent reports fetching [#350][hp1] and [#441][m43] on 2026-09-13:
both were OPEN with no comments. This is supplied lookup provenance, not a
lookup performed by this writer. These issues delimit the separate direct
publication work; they do not expand this catalog's scope or supply incidents.

The owner-approved specification is published as
[Spec: Canonical output buffer removal](https://github.com/ahrav/eidnara/issues/554).
Read-back confirms its body exactly matches the local
[specification](../../specifications/canonical-output-fast-path.md), with SHA-256
`ca3df7194f0a39f3ce454b69a39326363c5be819bafc233245c657e8719a93b1`.
Publication creates one specification, not implementation tickets. This
catalog, its evidence, and the specification copy are tracked with the U0
harness.

Seven records remain **Exercised: not yet**; S5 and C2 are **partial** after
the plan U0 harness landed at `c1dafa76` and captured the
[baseline record](evidence/u0-baseline-measurement.md) on the unchanged
canonicalizer. No candidate ran. Inspected pre-U0 tests remain **unaudited**.
Confidence describes the evidence for the obligation and its reachability,
not proof that an implementation satisfies it.

## System model by lens

| Lens | Verified model and consequence |
| --- | --- |
| Architecture and data flow | [Encoding][encode] serializes once into growable A, records spans, sorts every object, then unconditionally allocates and fills B at HEAD. The plan selects returning A only when every field permutation is identity. |
| Representation and compatibility | [Wire messages][message] replay retained parsed `Value` or serialize typed data. Originals are not raw input text. Existing decoded Rust `String` ordering in [sort_fields][sort] is preserved; the host wire contract does not define a new canonical collation. |
| State, cache, and persistence | The [transform caller][caller] passes the output cache; [default configuration][config] enables compaction. A [clean positive hit][cache] reuses served owners. Completed [prepared-page replay][replay] is a distinct in-memory replay path. Neither is a new durable replay guarantee. |
| Ownership and concurrency | [ServedMessage][constructor] owns message, canonical bytes, identity, and receipts. A returned Vec remains live through receipt construction and hashing before Arc conversion. Cache snapshots and response owners can outlive a cache entry. |
| Safety and resources | [Formatter tables][spans] give unique increasing field starts. Compact punctuation and recursive [copying][copy] supply the identity lemma. Removing B does not remove A growth, decoded-key work, Arc conversion, response assembly, or arena copies. |
| Failure and degradation | The serialization `?` precedes finalization. [Preparation][dispatch] preserves source-specific length and error behavior. [Settlement][settlement] measures, checks cancellation, reserves, checks cancellation, and writes. |
| Liveness and coordination | This is synchronous work before reservation. It adds no distributed coordination, asynchronous ownership, recovery loop, or liveness state machine. Preserve existing cancellation cuts and [best-effort wire cancellation][wire-cancel]; no new transport fault campaign is required. |
| History and product context | The output lens inspected bounded history around span serialization and key-decoding work. Those changes are not incident reproductions or measured speedups. Cold canonical misses can benefit; warm hits already bypass construction. Actual population frequencies are unknown. |
| Wildcard | Default [HarnessMeta][meta] emits `{}`; [a typed block parent][block] can be ordered while its tagged child is not. Untouched block originals survive `content_mut`. These refine witnesses, not scope. See [dispositions][wildcard]. |

The [canonicalizer][canonical-lens], [ownership][ownership-lens], and
[output-verification][output-lens] reports retain their separate discovery
reasoning. Their candidates are refined here: output diagnostics are not
marked partially exercised, per-object production counters are not required,
and no general span-geometry validator or new transport contract is proposed.

### Reachability and oracle limits

Reachability follows the nonvacuous obligation, not the location of its test.
Six safety records concern default production paths: S1's core permutation
obligation is reached in production even though duplicate-key probes are
private. S4 is `test-only` because exercising its error clause requires a
deliberately failing private generic `Serialize` source; its single-pass rule
also constrains production.
The canonicalization campaign is `test-only` as a combined portfolio because
it requires those private error and duplicate-key probes. The cache campaign
covers default production situations, constructed by tests.

The [manifest][serde-feature] requests `serde_json/raw_value`, not
`preserve_order`. The [locked serde_json 1.0.151 entry][serde-lock] lists
`itoa`, `memchr`, `serde`, `serde_core`, and `zmij`, with no `indexmap`.
Together these support the inspected locked sorted-map assumption. The U0
paired-run provenance retains the `cargo tree -e features` lines for `daemon`,
`memory-store`, `serde`, and `serde_json`. The algorithm must remain independent
of that feature. `to_vec(to_value(message))` is a canonical
oracle only with sorted `Value` iteration and unique keys. Use frozen literals
and independently decoded emitted-key permutations as unconditional witnesses.
Do not route duplicate keys through `Value`, require `preserve_order` off,
authorize the fast path from `original().is_some()`, or admit arbitrary
`RawValue` fragments through this facade.

## Index

| ID | Property | Type | Reachability | Check | Exercised |
| --- | --- | --- | --- | --- | --- |
| S1 | [served-field-change-flag-matches-stable-permutation](#served-field-change-flag-matches-stable-permutation) | safety | default-production | always | not yet |
| S2 | [served-order-decision-visits-every-object](#served-order-decision-visits-every-object) | safety | default-production | always | not yet |
| S3 | [served-unchanged-span-copy-is-identity](#served-unchanged-span-copy-is-identity) | safety | default-production | always | not yet |
| S4 | [served-serialization-is-single-pass-and-error-terminal](#served-serialization-is-single-pass-and-error-terminal) | safety | test-only | always | not yet |
| S5 | [served-canonical-return-retains-a-without-b](#served-canonical-return-retains-a-without-b) | safety | default-production | always | partial |
| S6 | [served-output-cache-hit-skips-construction](#served-output-cache-hit-skips-construction) | safety | default-production | always-or-unreached | not yet |
| S7 | [prepared-output-diagnostics-preserved](#prepared-output-diagnostics-preserved) | safety | default-production | always | not yet |
| C1 | [served-canonicalization-campaign-reaches-risk-classes](#served-canonicalization-campaign-reaches-risk-classes) | reachability | test-only | sometimes | not yet |
| C2 | [served-cache-campaign-reaches-miss-and-hit](#served-cache-campaign-reaches-miss-and-hit) | reachability | default-production | sometimes | partial |

## Records

### served-field-change-flag-matches-stable-permutation

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - The changed flag is absent at HEAD; no candidate runs exist.
Guarantee: Sorting reports a change exactly when the stable decoded-key
permutation differs from the recorded field order.
Check: `always` - For each object's complete source-ordered field descriptors
F, independently sort indices by `(decoded key, original index)` to obtain P.
Require the result to equal F indexed by P, with unchanged descriptor contents,
and `changed == (P != identity)`. Require the same flag to equal the negation
of strictly increasing result field starts. Empty and singleton sequences are
increasing. Every evaluated object owes this, not only sampled final bytes.
Fault/timing angle: Escaped spellings, prefix keys, or unstable equal-key
handling can produce an incorrect permutation or flag without a timing fault.
Required faults and enabling state: All six three-key permutations, empty and
singleton objects, ordered/disordered escaped keys, empty/prefix/control/quote/
backslash/Unicode keys, and private equal-key probes with distinct value tags.
Production reaches sorting through [encode][encode]; duplicate emission is
test-only. Keep the fewer-than-two-fields guard before key decoding.
Confidence: high - [Evidence](evidence/served-field-change-flag-matches-stable-permutation.md).
Recording, ordering, and unique starts are source-verified; flag behavior is
an unimplemented plan obligation at this HEAD.
Existing check: [Key and scalar tests](existing-checks.md#canonicalizer-and-guards)
are unaudited; no flag or stable duplicate-key check was found.
Impact: False negatives threaten B1 bytes; false positives defeat plan R1's
no-B allocation requirement while potentially preserving every byte.
Open questions:
- The changed result and pre-sort descriptors need private test observation.

### served-order-decision-visits-every-object

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No candidate aggregate-decision witness has run.
Guarantee: Every recorded object is sorted exactly once before the aggregate
decision can authorize returning A.
Check: `always` - Each completed encode preserves object-table order/ranges,
applies the expected stable field permutation to every object, and aggregates
the disjunction of their independent change predicates without skipping any
object. Only an all-false aggregate permits returning A. Verify the real loop
in source and use literal late-disorder output witnesses; per-object counters
are optional test observations, not required production instrumentation.
Fault/timing angle: A short-circuit after early disorder skips later siblings
or descendants; checking only an enclosing object misses child disorder.
Required faults and enabling state: Early and late disordered objects with
ordered/empty objects between them; an ordered typed block parent with a
disordered tagged child; and a private ordered-root/disordered-child source.
The exact ordered `WireMessage` root case is test-only under the pinned
sorted-Value assumption, unlike the production block-parent witness.
The production witness requires child traversal, but its typed message root
is already disordered. Only the private ordered-root witness isolates a
false-negative root-only aggregate decision.
Confidence: high - [Evidence](evidence/served-order-decision-visits-every-object.md).
The loop, object recording, and typed child field order are verified at HEAD.
Existing check: [Nested and counted literals](existing-checks.md#canonicalizer-and-guards)
are unaudited; no aggregate-flag assertion was found.
Impact: B1 ordering can fail even when the first object's local flag is correct.
Open questions:
- Add the smallest late-disorder witness that rejects short-circuiting without
  introducing a parallel loop or observation framework.

### served-unchanged-span-copy-is-identity

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No direct unchanged-table copy comparison has run.
Guarantee: Copying completed compact serialization through unchanged recorded
field spans reproduces the selected source range byte for byte.
Check: `always` - With A and unchanged source-order tables captured from the
real successful encode, require the real copier's whole-buffer output to
equal A in bytes and length; complete object/field subranges must equal their
source slices where tested. Compact braces, commas, and field boundaries give
the recursive identity proof. This lemma applies even when A needs sorting;
it is not merely a canonical-output comparison.
Fault/timing angle: Wrong punctuation, nested traversal, or resume position
can break equivalence between returning A and the old unchanged-copy path.
Required faults and enabling state: Real formatter tables for empty/singleton
objects, arrays of objects, nesting, scalar gaps, punctuation inside strings,
escapes, Unicode, and numeric edge forms. Use a private test-only observation
of real encode after optimization; do not duplicate serializer setup, add a
production hook, or require a general geometry validator.
Confidence: high - [Evidence](evidence/served-unchanged-span-copy-is-identity.md).
Formatter boundaries and recursive compact-punctuation reasoning are verified;
the direct observation is missing.
Existing check: [Formatter guards and byte fixtures](existing-checks.md#canonicalizer-and-guards)
are unaudited; no direct unchanged-table identity check was found.
Impact: A correct no-change flag alone would not justify omitting the copier.
Open questions:
- Retain the actual tables for a scoped private test without a second encode.

### served-serialization-is-single-pass-and-error-terminal

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - Success checks were inspected only; error probes are absent.
Guarantee: Each encode traverses its source once and enters finalization only
after serialization succeeds.
Check: `always` - Count root and distinct child emission sites independently.
Success visits exactly one expected traversal; failure visits exactly its
expected prefix, returns its error, and enters no finalization, sorting,
return-buffer selection, or reorder copy. Observe finalization before either
success return, not only inside the copier. This is a forbidden-state check,
not `unreachable` coverage of an invented code point.
Fault/timing angle: Raise a serializer error before writing and after a
nonempty prefix with an open nested object.
Required faults and enabling state: Private counted/failing generic Serialize
sources. The production WireMessage facade expects serialization to succeed;
the constructed error is not claimed production-reachable. Canonical and
unordered successful controls both owe one traversal.
Confidence: high - [Evidence](evidence/served-serialization-is-single-pass-and-error-terminal.md).
The single serialization and error-return boundary are verified in source.
Existing check: [Counted success and source guard](existing-checks.md)
are unaudited; no failing-prefix/finalization probe was found.
Impact: A second traversal can observe stateful input twice; finalizing partial
tables can panic or replace the original error with partial output.
Open questions:
- Add a private finalization observation that also detects erroneous early return.

### served-canonical-return-retains-a-without-b

Type: safety
Reachability: default-production
Status: active
Exercised: partial - The U0 baseline observer records one exact-N B allocation
and A's release for every population at `c1dafa76`; candidate ownership is
unmeasured.
Guarantee: A successful encode with only identity field permutations returns
A by ownership transfer without B or replacement output-sized scratch.
Check: `always` - Independently establish identity permutations, then require
the returned Vec to retain A's allocation lifetime, pointer, length, and
capacity from the post-serialization boundary, without shrink or reallocation.
Require zero B allocation attempts and zero logical reorder-output bytes.
An isolated absolute allocation/size ledger must attribute all output-sized
allocations to A's growth chain, excluding replacement scratch. Pointer
equality or the existing allocation slope alone is insufficient.
Fault/timing angle: Hidden allocate/copy/discard work can preserve bytes and
the returned pointer while defeating the optimization.
Required faults and enabling state: A cold canonical miss with a large scalar
and small span/key metadata, including ordered escaped keys. Construct fixtures
and reference bytes outside thread-owned, nonallocating recording. Follow the
explicit libtest and nextest isolation commands in [the evidence](evidence/served-canonical-return-retains-a-without-b.md).
Confidence: high - [Evidence](evidence/served-canonical-return-retains-a-without-b.md).
The B site and later Arc boundary are verified; no allocation saving is claimed.
Existing check: [Allocation slope](existing-checks.md#allocation-and-measurement)
is unaudited; no absolute B/lifetime/copy oracle was found.
Impact: Plan R1 can fail while every B1 byte assertion passes. Longer-lived A
slack can also raise full-constructor peak memory despite local savings.
Open questions:
- Implement isolated recording without counting harness allocations.
- U0 must establish and verify the full-constructor observer in an isolated,
  filtered in-crate test using the existing private constructor. Search/reuse
  allocator fixtures first; the integration facade covers only canonicalization.
  No public wrapper or production hook is needed. If compatible test-only
  observation is unavailable, stop U0 for seam approval rather than weaken the
  peak gate. The feasible route is not executed proof.
- Measure A through receipt construction, hashing, and Arc conversion; investigate
  increased constructor peak residency before landing. B1 preserves final Arc
  payload length/bytes; unchanged retained accounting is not a global RSS bound.

### served-output-cache-hit-skips-construction

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No independent hit-path construction observation has run.
Guarantee: Selecting a clean matching positive output-cache entry reuses its
owned artifacts without constructing or canonicalizing that message.
Check: `always-or-unreached` - For each selected positive hit, require zero
constructor and encoder call deltas for that item and pointer-equal message,
canonical-byte, identity, and fingerprint Arcs. The optional hit path owes
these checks when selected; C2 separately requires its enabling situation.
Fault/timing angle: The relevant transition is miss, insertion, then hit;
there is no injected concurrency fault.
Required faults and enabling state: Same session, revert epoch, key and
identity, clean item, and a retained positive entry. Exercise synthetic and
live-tail callers; `Some(None)` omission is not a positive hit. Exclude the
test-only fresh differential from the observation interval.
Live-tail lookup increments hits for `Some(None)` too; the synthetic helper
flattens that result into a miss. Existing hit counters cannot prove a warm
positive item: require `Some(Some(served))` independently.
Confidence: high - [Evidence](evidence/served-output-cache-hit-skips-construction.md).
Both cache-hit branches and Arc ownership are verified at HEAD.
Existing check: [Replay and invalidation checks](existing-checks.md#served-ownership-and-cache)
are unaudited; their reuse counters are not independent constructor spies.
Impact: Equivalent reconstruction can restore hashing and allocation on warm
requests while passing byte comparisons.
Open questions:
- Extend existing private tests with scoped call and Arc observations, not a
  production timing field or a new cache API.

### prepared-output-diagnostics-preserved

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - Existing output and cancellation tests were inspected only.
Guarantee: Preparation preserves successful lengths and source-path-specific
failure diagnostics and settlement order.
Check: `always` - Compare the same PreparedSource variant, cap, measurement
call partition, and write call partition before and after the change. Success
writes exactly the measured length. Failures preserve error variants, codes,
messages and length fields; cap refusal precedes reservation, cancellation at
either existing cut or denial writes no body, and write failure returns no
Response. Different variants or chunk partitions need not report equal errors.
Fault/timing angle: First cap crossing, arithmetic overflow, inconsistent
segment length, cancellation around reservation, denial, and writer failure.
Required faults and enabling state: Existing public measure/write and private
settlement seams, with fixed partition-aware fixtures for Json, Exact, and
Transform. CountingWriter reports the first crossing length, not the eventual
full length. Synthetic inconsistent lengths are test-only witnesses.
Confidence: high - [Evidence](evidence/prepared-output-diagnostics-preserved.md).
Error mappings, counting, and settlement order are verified against HEAD.
Existing check: [Prepared-output and settlement checks](existing-checks.md#prepared-output-and-settlement)
are unaudited; no complete variant/partition diagnostic matrix was found.
Impact: Unchanged successful bytes can hide changed refusal or cancellation
behavior. A local unset terminal variable is not ring-publication evidence.
Open questions:
- Add only missing path-specific diagnostic assertions within existing seams;
  do not introduce a new short-write or transport contract.

### served-canonicalization-campaign-reaches-risk-classes

Type: reachability
Reachability: test-only
Status: active
Exercised: not yet - No accumulated situation markers have run on this revision.
Guarantee: A canonicalization campaign constructs every declared key, nesting,
identity-copy, and serialization-error risk class.
Check: `sometimes` - Accumulate independent precondition markers across the
campaign and require every class in [the C1 witness set](evidence/served-canonicalization-campaign-reaches-risk-classes.md#what-a-test-must-construct)
at completion, including all six permutations and both error positions. Report
each missing marker separately, including incomplete composite members. This
is an AND over required situations, not one OR over whichever case appeared;
it records setup and emitted input, never a wrong flag or corrupted bytes.
Fault/timing angle: Generic serializer errors occur at explicit emission cuts;
other risk classes require input shape, not timing faults.
Required faults and enabling state: Empty/singleton, raw and escaped order,
prefix/control/Unicode/equal keys, late and child-only disorder, real-table
identity cases, numeric forms, both success paths, and errors before writing
and with an open nested object. Private generic probes make the joint set
test-only; production witnesses are distinguished in the evidence.
Confidence: high - [Evidence](evidence/served-canonicalization-campaign-reaches-risk-classes.md).
Each precondition is constructible through verified private or facade seams;
none is reported as observed at runtime.
Existing check: [Individual fixtures](existing-checks.md#canonicalizer-and-guards)
are unaudited; no accumulated campaign requirement was found.
Impact: Safety tests can remain green while never reaching escaped identity,
late disorder, empty metadata, or partial serialization.
Open questions:
- Implement constant globally unique markers with campaign-wide accumulation
  rather than requiring incompatible situations in a single encode.

### served-cache-campaign-reaches-miss-and-hit

Type: reachability
Reachability: default-production
Status: active
Exercised: partial - The U0 driver constructs cold canonical misses, typed and
edited unordered misses, and warm positive hits and records their frequencies;
completed-output page replay is not constructed.
Guarantee: A cache campaign constructs cold canonical misses, typed and edited
disordered misses, warm positive hits, and separate prepared-result replay.
Check: `sometimes` - Accumulate all independent [C2 preconditions](evidence/served-cache-campaign-reaches-miss-and-hit.md#what-a-test-must-construct)
and require each at campaign completion, reporting each missing marker and
incomplete composite member separately. Classify canonicality from emitted
keys, misses from absent/ineligible entries, positive hits from eligible owned
entries, and prepared replay from matching completed-attempt metadata. Do not
use no-B, no-constructor, byte equality, or forbidden allocation as markers.
Fault/timing angle: Cache insertion/warming and matching completed-page replay
are distinct state transitions, with no new transport fault.
Required faults and enabling state: A cold retained canonical source including
escaped keys; typed and one-block-edited disordered sources; clean matching
positive entries in both caller paths; a matching completed PreparedOutput.
These situations use default production paths, not restart durability.
Confidence: high - [Evidence](evidence/served-cache-campaign-reaches-miss-and-hit.md).
Production caller, miss/hit branches, and completed-page replay are verified.
Existing check: [Cache fixtures and cold/warm benchmarks](existing-checks.md)
are unaudited; no combined situation-coverage assertion was found.
Impact: Warm-only measurements can conceal an unexercised optimization, while
miss-only tests cannot establish that warm bypass remains intact.
Open questions:
- The U0 driver records canonical/miss/hit frequencies for the declared
  100/1,000-message fixtures only; those sizes are not empirical evidence of a
  production population mix. Completed-output page replay is still not
  constructed.

## Relationships and handoff

| Record or owner | Relationship and boundary |
| --- | --- |
| S1 → S2 → S5 | Local permutation correctness feeds a complete aggregate decision, which permits A transfer. Correct bytes alone do not imply the S5 resource result. |
| S3 → S5 | Unchanged-copy identity connects the old copier to A return. It does not prove that the aggregate inspected every object. |
| S4 → S1–S5 | Successful finalization requires one completed serialization; incomplete spans must never enter either return path. |
| C1 → S1–S5; C2 → S5–S6 | Campaign witnesses prevent missing-input vacuity. They are not safety or allocation oracles. |
| S7 → plan R4 | Length/error/cancellation behavior is preserved at preparation and owned-output settlement, not newly specified for transport. |
| [B1: derived-artifacts-are-ownership-independent][b1] | Reuse the broad owner for canonical bytes, hashes, block receipts, unknown-field/edit behavior, and retained served segments. Do not duplicate or rewrite it. |
| [T1: arena residency][t1]; [T2: atomic payload copies][t2] | Preserve the existing admission/reclamation and atomic-copy boundaries. No new transport campaign is required by this local optimization. |
| [T3: direct-frame publication][t3]; [T4: direct-frame lifetime witness][t4] | Keep with HP1/#350 and #441. Neither is delivered or exercised here; their test-only classification is unchanged. |
| [W1: production-shape measurement][w1]; [W2: timing fields][w2] | Both stay invalidated. Plan U0/U4 measurement is an independent obligation. No extra production timing field is requested. |

Every S1–S7 and C1–C2 record goes to `/testing:test-strategy`, with its linked
evidence file as the handoff. Existing test strength goes separately to
`/testing:invariant-test-review`; production guard strength goes to
`/low-level-systems:defensive-assertions-and-invariant-guards`. No new seeded
distributed simulation is justified. Domain provenance is the settled plan,
the three lane reports, the initial independent agent evaluation, this
writer's pinned-source refinements, and the completed independent final
portfolio evaluation. This is discovery acceptance, not implementation approval.

Deliverables: [existing checks](existing-checks.md), [fault map](fault-map.md),
[evaluation status](portfolio-evaluation.md), nine evidence files, and
[baseline](_lenses/portfolio-baseline.md)/[wildcard][wildcard] traces. Final
independent analyst evaluation `ses_f675203e4ffevgEH7CseOxb6jK` is complete:
accepted with no blockers. All eight findings are dispositioned in the
evaluation report. The U0 baseline measurement has run on the unchanged
canonicalizer; candidate observations remain unrun.
Semantics distribution: six `always`, one `always-or-unreached`, two
`sometimes`; no `reachable`, `unreachable`, or liveness records.

[plan]: ../../plans/2026-09-13-0030-perf-canonical-output-direct-frame-plan.md
[hp1]: https://github.com/ahrav/eidnara/issues/350
[m43]: https://github.com/ahrav/eidnara/issues/441
[canonical-lens]: _lenses/canonicalizer.md
[ownership-lens]: _lenses/served-ownership.md
[output-lens]: _lenses/output-verification.md
[wildcard]: _lenses/wildcard.md
[b1]: ../hot-path-optimization/latency-audit/catalog.md#derived-artifacts-are-ownership-independent
[t1]: ../hot-path-optimization/latency-audit/catalog.md#arena-residency-is-bounded-by-admission-and-one-punch-batch
[t2]: ../hot-path-optimization/latency-audit/catalog.md#arena-payload-copies-keep-the-address-derived-atomic-shape
[t3]: ../hot-path-optimization/latency-audit/catalog.md#direct-frame-publishes-declared-length-or-nothing-and-holds-its-charges
[t4]: ../hot-path-optimization/latency-audit/catalog.md#direct-frame-outlives-its-handler-before-publication
[w1]: ../hot-path-optimization/latency-audit/catalog.md#optimized-stage-is-measured-at-production-shape
[w2]: ../hot-path-optimization/latency-audit/catalog.md#stage-timing-fields-keep-their-boundaries
[encode]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L121-L142
[sort]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L144-L164
[spans]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L24-L81
[copy]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L84-L109
[constructor]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L144-L224
[caller]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L8492-L8499
[config]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/config.rs#L116-L125
[serde-feature]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/Cargo.toml#L47
[serde-lock]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/Cargo.lock#L2828-L2839
[cache]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L10392-L10432
[replay]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L9680-L9701
[message]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L99-L162
[meta]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L73-L123
[block]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L243-L278
[dispatch]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/dispatch.rs#L237-L408
[settlement]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L12360-L12431
[wire-cancel]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/docs/host-wire-protocol.md#L765-L798

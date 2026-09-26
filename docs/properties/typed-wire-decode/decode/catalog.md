# Typed wire decode: decode, ownership, and mutation properties

## Scope and provenance

System: `/local/home/ahrav/scratch/eidnara`. Source baseline:
`2e4433e6b511ae74944df8a9669c428e73915d29` (HEAD). Inspection date: 2026-09-13.
These are discovery artifacts, not implementation or execution evidence.

The user supplies the settled
[typed-wire plan](../../../plans/2026-09-13-0104-perf-typed-wire-decode-plan.md),
the [host wire contract](../../../host-wire-protocol.md), latency-audit
A1-A3/B1/W1, and related GitHub issues as leads. No separate incident reports
or external repositories are supplied. The explicit amendment supplied with
the independent portfolio findings completes this scope; no additional
interview is needed.
The plan is absent from HEAD and is **worktree-only accepted material**, with
SHA-256 `badf0d718366bd627d453498576935ba2fd3292cfe5701b7020fa09a07c52f32`.
Plan references below use requirement/decision names, not unverified line numbers.

Relevant HEAD sources are compared with plan baseline
`e451a2b470ae8663b4613ca04f019a30b6d7df53`. Memory-store wire types, daemon
wire/transform/served_json/sidecar/meter, the plugin encoder, and inspected
direct-host/page fixtures match. Daemon lib.rs differs only by the added
`pub mod search_seed;` line, which shifts following daemon references by +1.
This offset does not invalidate all plan references. Local main lacks
metered_decode and is not a valid substitute baseline. No branch is switched.

The initial 2026-09-13 discovery records unrelated staged and unstaged edits
in daemon lib.rs. Every citation to it uses
`git show HEAD:crates/daemon/src/lib.rs`. Other cited committed files are
checked against HEAD before using worktree line numbers. The plan's reattach
test pointers are corrected to `wire.rs:1708` and `:1749`; stale-edit coverage
is at `transform.rs:13714`. Worktree-only neighboring catalogs are labeled
below and are not implementation evidence.

### Consulted external leads

All issue bodies and comments are read through `gh issue view` on the
inspection date. They establish requested work, not verified incidents.

| Source | Why consulted |
| --- | --- |
| [350](https://github.com/ahrav/eidnara/issues/350) | Inherited wire, durability, and preservation constraints. |
| [435](https://github.com/ahrav/eidnara/issues/435) | Direct decode, page convergence, and frozen outcome obligations. |
| [436](https://github.com/ahrav/eidnara/issues/436) | Distinguish admission refusal from invalid typed decode; accounting stays elsewhere. |
| [438](https://github.com/ahrav/eidnara/issues/438) | Owned captures and blocking-work lifecycle prerequisites. |
| [441](https://github.com/ahrav/eidnara/issues/441) | Keep egress framing outside this decode slice. |
| [524](https://github.com/ahrav/eidnara/issues/524) | Owned decode output permits later private-input release; not evidence of release today. |

The explicit `/to-spec` context supplies settled decisions. Publication
requires final approval of the complete specification and exact tracker
mutation. It is not forbidden by this scope. The inspected `ask-skills` routing
contract selects the installed `property-discovery-and-catalog` owner for
this enrichment. No new spec, tracker item, or implementation is produced.

### Existing catalog inventory and ownership

Existing catalogs are inventoried before new records are written. This slice
adds precise acceptance and ownership witnesses to these canonical obligations:

| Existing artifact | Relationship |
| --- | --- |
| [A2: route and typed decode](../../hot-path-optimization/latency-audit/catalog.md#route-and-typed-decode-are-independent-of-entry-path) | Owns broad route/decode equivalence. The fallback and page records specialize its new envelope risks. |
| [B1: ownership-independent artifacts](../../hot-path-optimization/latency-audit/catalog.md#derived-artifacts-are-ownership-independent) | Owns full projection/native-output equivalence. This slice adds owner-drop, alias, and typed-mutation conditions. |
| [A1: admission](../../hot-path-optimization/latency-audit/catalog.md#admission-chain-charges-before-decode-and-refuses-effect-free), [A3: shortfall](../../hot-path-optimization/latency-audit/catalog.md#scratch-pool-shortfall-reaches-the-parse-reservation) | Retain canonical budget and refusal ownership; no duplicate accounting records. |
| [W1](../../hot-path-optimization/latency-audit/catalog.md#optimized-stage-is-measured-at-production-shape) | Invalidated at HEAD. Plan U4 still names it; reconciliation is pending. |
| [Daemon handlers](../../daemon/handlers/catalog.md) | Owns page collection, replay, expiry, and route lifecycle. This slice preserves the digest's raw input boundary. |
| [Memory store](../../memory-store/catalog.md), [transform](../../daemon/transform/catalog.md) | Own durable state and transform correctness beyond decode representation. |
| [Independent payload pools](../../independent-payload-pools/catalog.md#private-decode-input-stability) | Worktree-only catalog; adjacent transport/private-input ownership, not evidence that its replacement exists. |
| [Transform edit responses TE08](../../transform-edit-responses/catalog.md#cached-canonical-prefix-preserves-exact-ingress) | Worktree-only catalog; exact captured CK ingress, including unknown fields, conflicts with accepted R3. This spec's owner preserves R3 and records the integration owner's reconciliation before combining the work. |

Identity hashes, history_summarizer migration, meter constants, resident budgets, and
performance measurements belong to separate agents. They receive named
handoffs rather than new records here. No tests, builds, or benchmarks run.

## System model and lens coverage

The ordinary handler reads private body bytes, checks admission, probes route
and page keys, runs the compatibility walk, and attempts TransformRequest
decode. Invalid typed decoding falls through to the original bytes' tree
decode. Admission refusal does not fall back. Pages and other routes use a
Value tree. Page arrays are hashed before typed CK conversion.

HEAD wire structs retain original trees. The accepted replacement removes
those envelopes but keeps payload Values and owned strings. Requests,
projections, and prefix shells remain shared by Arc across turns. Mutations
must act on typed state without a second replay authority.

All 12 system-model lenses and 11 property lenses run. The 23 reports remain
under [_lenses](_lenses/). Wildcards run after all non-wildcard passes.
Parallel delegation is attempted but blocked by the harness depth limit;
passes therefore run inline with separate attention focuses. They are not
independent corroboration. Fresh portfolio evaluation is
[completed](portfolio-evaluation.md) on 2026-09-13 by independent analyst
`ses_f6756093fffeVjNp36S3E8pKrM`, with findings supplied by the user and
dispositioned in this decode part. All seven claims remain unexercised;
existing checks remain unaudited.

Scope survey: HEAD memory-store lib.rs has 26,806 lines, daemon lib.rs 39,026,
transform.rs 29,195, and wire.rs 1,863. Wire representation is concentrated
in memory-store lines 72-447. Direct parsing tests and shared-shell tests are
clustered in large modules; the inventory names individual checks rather than
using module test counts as evidence. The 376-line wire definition window
(memory-store lib.rs:72-447) contains zero assert/debug_assert macros; serde
conversion supplies its field validation. No envelope-tree absence guard exists.

## Accepted decisions and contradictions

1. KTD1/R3 replace lossless original replay with typed state. HEAD comments,
   sibling-unknown tests, and stale-edit goldens contradict that prospective
   contract. Preserve the accepted decision and update those oracles during
   implementation, not in this discovery pass.
2. R2 preserves semantic outcomes, with an accepted lane change for duplicate
   message/block fields. A2's historical lane list is not immutable for those
   added witnesses. Literal duplicate keys must survive corpus construction.
3. Raw-value-token behavior may change when discarded CK fields cease being
   re-deserialized as Value. This is a source-backed hypothesis requiring
   baseline/final evidence, not a confirmed regression or an accepted exception.
   The specification requires stop-and-report for any observed acceptance
   change. No compatibility exception is authorized. A separate review's
   confirmed-defect wording is not adopted without a discriminating run.
4. KTD3 keeps payload Values, native messages, tail_delta, and owned types.
   Optional Value fields still normalize field-level null to None. Unknown
   enum tags and invalid known fields do not become ignorable data.
5. KTD5 keeps the page tree and its pre-normalization digest. R3 applies at
   typed conversion, not before page validation. Non-transform echo retains
   unknown data in its request tree.
6. The plan says explicit-null serializer_profile is covered by A2. The
   inspected corpus has missing and unknown profile cases, but no such null
   case. The wrapper's Option semantics remain accepted and need a witness.
   This is boundary preservation, not authorization for the deferred
   TransformRequestWire wrapper refactor.
7. Plan U4 names W1, but W1 is invalidated at HEAD. Keep payoff ownership
   unresolved for the measurement agent; do not reactivate W1 here.
8. Worktree-only TE08 requires exact captured CK ingress, including unknown
   fields, whereas R3 discards those fields at typed conversion. R3 remains
   authoritative for this specification. Its owner records the conflict and
   requires the integration owner to reconcile TE08 before integration.
   That reconciliation remains open; the other catalog is not edited here.

The host contract keeps routed bodies opaque (`docs/host-wire-protocol.md:337`).
Its strict duplicate handling for channel 0 and LocalEmbeddings is not a mandate to
reject duplicate CK fields. No transport fallback or protocol version change
is proposed by this catalog.

## Index

| Record | Type | Check | Extends |
| --- | --- | --- | --- |
| [envelope-decode-has-no-retained-tree](#envelope-decode-has-no-retained-tree) | safety | always | R1/KTD1 |
| [typed-failure-preserves-tree-outcome](#typed-failure-preserves-tree-outcome) | safety | always | A2/R2 |
| [unknown-envelope-fields-do-not-erase-payload-values](#unknown-envelope-fields-do-not-erase-payload-values) | safety | always | R3/KTD3 |
| [page-and-other-routes-retain-tree-semantics](#page-and-other-routes-retain-tree-semantics) | safety | always | A2/KTD5 |
| [decoded-snapshots-own-and-share-prefixes](#decoded-snapshots-own-and-share-prefixes) | safety | always | B1/KTD3 |
| [typed-mutation-is-visible-and-copy-isolated](#typed-mutation-is-visible-and-copy-isolated) | safety | always | B1/KTD1 |
| [nested-duplicate-fallback-is-exercised](#nested-duplicate-fallback-is-exercised) | reachability | sometimes | A2/R2 |

Six `always` and one `sometimes` records are active. No new liveness deadline
is justified for this synchronous decode boundary. No forbidden code-point
claim needs `unreachable`. The `sometimes` record contains two independently
required marker checks; a hit for one cannot satisfy the other.

## Records

### envelope-decode-has-no-retained-tree

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `WireMessage` and `WireBlock` derive serde and hold no
envelope `Value`; `source_has_no_envelope_tree_or_replay_entry` in
`crates/daemon/tests/typed_wire_decode_allocations.rs` checks the struct
definitions and the crate-wide absence of the replay entries, and
`message_decode_stays_within_the_allocation_budget` measures the decode
(434 events, peak 1.46 J) against a restored-envelope control (2,185 events,
peak 4.2 J).
Guarantee: WireMessage and WireBlock decode and serialize from owned typed
fields without constructing or retaining full Value envelopes.
Check: `always` - Inspect both definitions and serde paths for derived
Serialize/Deserialize, no original or replacement envelope cache, no envelope
Value round trip, and only the accepted payload Value fields; this structural
invariant must hold for every decoding entry, while the outer page tree and
serde enum buffering remain permitted.
Fault/timing angle: A representation refactor accidentally preserves replay
state behind a different field or helper.
Required faults and enabling state: Both direct and from_value entrypoints,
mixed wire variants, and a source inspection of the final types and their
serializers.
Confidence: high - [evidence](evidence/envelope-decode-has-no-retained-tree.md).
Accepted contract and contradicting HEAD custom serde are inspected;
satisfaction is not claimed.
Existing check: None for envelope-tree absence; sibling replay assertions in
memory-store lib.rs:16096-16112 and wire.rs:1708-1745 are unaudited and pin the
prior shape.
Impact: The retained-copy cost remains and a second serialization authority
survives; R1/KTD1 are unmet.
Open questions:

- What final source-bound check accompanies the accounting agent's allocation
  witness without treating an allocation count as proof of representation?

### typed-failure-preserves-tree-outcome

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `unpaged_transform_bodies_reach_the_same_outcome_through_both_entry_paths`
and `both_lanes_charge_the_same_footprint_and_refuse_the_same_bodies` pass
unchanged over the 46-body corpus, and
`frozen_corpus_footprints_replay_with_only_string_charge_changes` replays every
body against its frozen terminals (see the resources catalog); the direct-lane
set is unchanged. Duplicate recognized envelope keys still fall to the tree lane
with the last value.
Guarantee: Replacing envelope serde preserves A2's routing, acceptance, and
semantic outcomes through invalid-typed-decode fallback, subject only to the
plan's explicit representation changes.
Check: `always` - For each literal body with nonbinding admission, compare the
body entry with independently frozen tree routing and decoding: equal terminal
class/code and equal known typed fields and retained payload Values; Invalid
may fall back, Refused may not, and no failed typed attempt dispatches; compare
the baseline separately so two identically changed lanes cannot pass together;
any raw-value-token acceptance change requires stop-and-report, with no
authorized compatibility exception.
Fault/timing angle: A derived duplicate-field error occurs after a valid
prefix, or an ignored field bypasses tree validation.
Required faults and enabling state: Recognized duplicates at
ingress/message/block/enum levels, retained-map duplicates, optional and
defaulted nulls, unknown fields/tags, malformed JSON, UTF-8, surrogate, range,
depth, and raw-value-token cases.
Confidence: high - [evidence](evidence/typed-failure-preserves-tree-outcome.md).
HEAD distinguishes Invalid and Refused and preserves a tree fallback; final
parity is untested.
Existing check: daemon lib.rs:19986-20117 and :20140-20181 compare decode and
entry outcomes; both unaudited, with nested CK witnesses missing.
Impact: A previously valid request fails, an invalid request dispatches, or
the same body receives a different typed result.
Open questions:

- Does dropping CK envelope re-reads change raw-value-token acceptance under
  discarded fields? The discriminating baseline/final run remains required;
  any change stops implementation for a report rather than an exception.
- Which malformed-input diagnostic strings must be frozen beyond stable
  error codes?

### unknown-envelope-fields-do-not-erase-payload-values

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `a_block_edit_leaves_its_sibling_unchanged_and_envelope_unknowns_are_discarded`
(memory-store), `reattach_shares_the_decoded_shell_and_unknown_envelope_fields_are_discarded`
(wire.rs), `overlay_canonicalizes_only_the_mutated_block` (transform.rs), and
`decoded_envelope_charges_only_typed_fields` (retained_size.rs) show message- and
block-level unknown fields discarded while payload values survive; a
placement-sensitive witness with unknown keys nested inside a retained payload
`Value` is not added.
Guarantee: Typed conversion discards unknown wire-envelope fields while
preserving the decoded values of every accepted payload Value field.
Check: `always` - After either lane accepts, compare each kept field with an
independently constructed typed expectation and require unknown typed-envelope
keys absent on reserialization; use Value equality for payloads and existing
Option/default normalization for field-level null, never recursively strip
unknown keys inside a kept Value.
Fault/timing angle: Normalization discards a payload subtree or retains a
whole envelope to preserve one extension.
Required faults and enabling state: Identically named sentinels at
CK/block/meta/origin/kind/output envelope positions and inside tool input,
JSON outputs, opaque raw/source/arc, media source, extras, native messages,
and tail_delta.
Confidence: high - [evidence](evidence/unknown-envelope-fields-do-not-erase-payload-values.md).
Field types and accepted classification are verified; old replay conflicts
are preserved.
Existing check: memory-store lib.rs:16096-16112 and daemon wire.rs:1708-1745
preserve unknown sibling data under the old contract; unaudited. Producer and
codec payload checks are in existing-checks.md.
Impact: Tool arguments, opaque content, native metadata, or media references
disappear, or R3 silently remains unimplemented.
Open questions:

Update, 2026-09-26: [#828](https://github.com/ahrav/eidnara/issues/828) deletes
`reattach_shares_the_decoded_shell_and_unknown_envelope_fields_are_discarded`
with prefix reattachment; decode-side unknown-field handling is unchanged. The
daemon projects every request from its full input. Citations of these symbols
here are historical at their stated baseline.

- Does every kept-field witness distinguish explicit field null from a null
  nested inside a Value?
- Which unlisted sender shapes rely on discarded typed-envelope extensions?
  The supplied producer inventory does not establish runtime exclusivity.
- How will the integration owner reconcile worktree-only TE08's exact captured
  ingress requirement with R3 before integration, while preserving R3?

### page-and-other-routes-retain-tree-semantics

Type: safety
Reachability: default-production
Status: active
Exercised: partial - the existing page-assembly and route tests pass unchanged
(`transform_decode_corpus` page cases and `assemble_transform_pages`); no new
page-digest witness was added, and paging code did not change.
Guarantee: Paging and non-transform routes retain their Value-tree behavior
and pages validate the original array digest before typed envelope normalization.
Check: `always` - For well-formed bodies that pass admission, any present page
key selects Tree; with a valid route, compare the page digest and assembly
outcome against frozen raw-Value
expectations including unknown CK fields and continuation items; require
non-transform route/outcome equality and echo-tree equality before and after,
while successful page assembly reaches the same typed normalization as
one-slice ingress.
Fault/timing angle: Typed conversion precedes digest validation or a shared
entry refactor moves another route onto the typed lane.
Required faults and enabling state: Complete and partial/null page envelopes,
altered unknown CK fields with an unchanged supplied digest, valid updated
digests, scalar changes, continuation items, malformed array fields, and
non-transform control bodies.
Confidence: high - [evidence](evidence/page-and-other-routes-retain-tree-semantics.md).
HEAD ordering and KTD5 are verified; no final-tree check runs.
Existing check: daemon lib.rs:19649-19706, :30224-30257, :30381-30389;
serialized_transform_pages.rs:11-156; all unaudited. The integration test is
ignored and needs generated input.
Impact: Valid pages fail digest validation, altered pages pass, or unrelated
routes lose data or change dispatch.
Open questions:

- Which frozen raw-page vector proves unknown CK data is still hashed before
  R3 drops it?

### decoded-snapshots-own-and-share-prefixes

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `wire.rs` sharing tests pass with the owned model
(`reattach_shares_the_decoded_shell_and_unknown_envelope_fields_are_discarded` and
the shared-shell checks at `wire.rs:1749-1814`), and
`decode_and_projection_fit_the_declared_pool` checks every projection block
points into the request's shells; the projection now shares the shell whenever
the effective synthetic flag matches (`wire.rs` `project_messages_from_state`).
No input-drop sequence was added. #828 deletes
`reattach_shares_the_decoded_shell_and_unknown_envelope_fields_are_discarded`
and the shared-shell checks at `wire.rs:1749-1814` with prefix reattachment;
`projection_rebuilds_only_the_shell_whose_synthetic_flag_changes` still checks
shell sharing through the projected blocks.
Guarantee: Decoded requests and retained projections own their data, remain
Send plus static, and share unchanged prefix shells without depending on the
body buffer's lifetime.
Check: `always` - Enforce Send + 'static for TransformRequest,
IngressMessages, and FlatProjection; after decoding, dropping input bytes and
selected cache/request owners leaves surviving typed values intact; repeated
valid prefix reattachment preserves Arc identity, and direct decoded shells
share with projection exactly when effective synthetic status is unchanged
under KTD1.
Fault/timing angle: Input-buffer release, snapshot fallback after cache
eviction, and projection owner drop while another block owner remains live.
Required faults and enabling state: Owned byte buffer with escapes and payload
Values, retained ready snapshot, nonempty prefix and suffix, matching/mismatching
synthetic flags, projection and native-cache eviction, and independently held
Arc owners.
Confidence: high - [evidence](evidence/decoded-snapshots-own-and-share-prefixes.md).
Owned field types and retention paths are verified; early production input
release is not claimed.
Existing check: daemon wire.rs:1749-1814 and :1818-1860; daemon
lib.rs:22886-23004; all unaudited, with byte-buffer-drop witness missing.
Impact: Buffer lifetime constrains request retention, prefix sharing is lost,
or surviving cached input becomes unusable.
Open questions:

- Can the final test distinguish projection-prefix reuse from ready-snapshot
  fallback after both relevant cache entries are evicted?

### typed-mutation-is-visible-and-copy-isolated

Type: safety
Reachability: default-production
Status: active
Exercised: yes - serialization walks the typed fields, so public-field edits
reach the wire without `mark_modified`; equality and `block_identity_digest`
cover `(kind, provider_extras)`; `a_block_edit_leaves_its_sibling_unchanged_and_envelope_unknowns_are_discarded`,
`overlay_canonicalizes_only_the_mutated_block`, `one_edited_block_message` in the
served-output fixtures, and the receipt-reuse test in `transform.rs` (blocks
differing only in discarded envelope fields are equal with equal digests) pass.
Guarantee: Serialization and equality reflect current typed fields, while
edits to one owned or copy-on-write shell leave retained peers and untouched
siblings unchanged.
Check: `always` - Compare an edited decoded value with a separately constructed
typed expectation: public role/origin/meta/extras and content/kind edits
serialize immediately; accessor calls without edits preserve value/equality;
block equality equals equality of (kind, provider_extras), message equality
equals its full typed field tuple, and other retained owners keep their prior
typed values and canonical typed serialization.
Fault/timing angle: Public-field edit without an invalidation call, no-op
mutable access, or Arc::make_mut while a projection retains the old shell.
Required faults and enabling state: Decoded and constructor-built equivalent
values, populated extras and meta, two siblings, multiple Arc owners, and
independently applied single-field edits.
Confidence: high - [evidence](evidence/typed-mutation-is-visible-and-copy-isolated.md).
Current dual-authority mechanism and contradictory golden are inspected;
KTD1 is prospective.
Existing check: memory-store lib.rs:16096-16112; daemon
transform.rs:13714-13755; daemon wire.rs:1785-1792; all unaudited and old replay
expectations require explicit revision.
Impact: A transformation is silently hidden, a cached prefix is changed
through an alias, or equality depends on decode history rather than typed state.
Open questions:

- Does the mutation matrix cover every public shell field and block extras
  without relying on removed mark_modified calls?
- Identity/equality handling of signed zero remains with the identity agent;
  typed equality alone must not imply byte equality.

### nested-duplicate-fallback-is-exercised

Type: reachability
Reachability: default-production
Status: active
Exercised: not yet - `parse_charge_covers_a_failed_typed_prefix_and_its_tree_fallback`
constructs a duplicate `mid` in the last message after a 4 MiB prefix and
asserts the walk accepts, the typed decode refuses, and the tree conversion
succeeds in one trace; the corpus's `duplicate nested key` body reaches the tree
lane with its last value. Both bodies repeat ingress `mid`, which is the
current-path control, not either required envelope-specific witness. Neither
constructs a repeated CK role or block kind, so neither required marker is
satisfied.
Guarantee: The compatibility campaign constructs successful tree recovery
from duplicate recognized fields inside message and block envelopes after
the typed attempt fails.
Check: `sometimes` - Independently require both fixed markers
typed-wire-decode-message-duplicate-fallback and
typed-wire-decode-block-duplicate-fallback, each observing literal unpaged
transform bytes with an accepted compatibility gate, Invalid typed decode,
successful independent tree conversion, and BodyLane::Tree; these are
reachable preconditions on a correct replacement, not a marker for
mismatched outcomes; maintain a separate witnessed bit for each marker and
require both bits true at campaign completion, so aggregate reporting cannot
mask an unfired marker.
Fault/timing angle: The duplicate occurs after other fields have decoded,
before any handler dispatch.
Required faults and enabling state: A valid literal body with repeated CK role
for one marker and repeated block kind for the other, valid last values, no
page fields, and nonbinding admission; retain duplicate ingress mid as a
current-path control.
Confidence: medium - [evidence](evidence/nested-duplicate-fallback-is-exercised.md).
Existing production fallback and current ingress-duplicate control are
verified; the new envelope-specific witnesses require KTD1.
Existing check: daemon lib.rs:19807-19812, :20045-20052, :20140-20181;
unaudited. None combines all new marker preconditions.
Impact: Safety comparisons pass without reaching the representation change's
principal fallback path.
Open questions:

- Will explicit gate/typed-attempt observations use the existing module-local
  seam or a narrowly extended direct-host fixture?
- An unfired marker needs investigation of corpus construction versus changed
  reachability, not automatic timeout inflation.

## Reachability evidence by record

`default-production` identifies the ordinary owning path. It does not assert
that the accepted replacement is implemented or that a witness has run.

| Record | Concrete reachability evidence and limit |
| --- | --- |
| envelope-decode-has-no-retained-tree | Handler body entry at daemon lib.rs:12153-12170 reaches wire deserialization through :12924; current implementation violates the prospective structural requirement. |
| typed-failure-preserves-tree-outcome | Ordinary invalid typed decode falls through at daemon lib.rs:12934-12941; no feature flag is needed. |
| unknown-envelope-fields-do-not-erase-payload-values | Both :12924 and :8116 reach wire structs; unknown keys can arrive in normal routed bodies, with accepted discard prospective. |
| page-and-other-routes-retain-tree-semantics | daemon lib.rs:8078-8084 and :12954-13046 route pages and other operations; an input page key, not a configuration flag, selects paging. |
| decoded-snapshots-own-and-share-prefixes | daemon lib.rs:4351-4400 retains and splices request prefixes under ordinary tail_delta traffic. |
| typed-mutation-is-visible-and-copy-isolated | Public setters/accessors are production APIs at memory-store lib.rs:164-228,282-323; projection ownership is ordinary wire.rs:89-114. |
| nested-duplicate-fallback-is-exercised | daemon lib.rs:12923-12951 is production fallback, with ingress-mid duplicates already characterized at :19809-19812; new CK/block cases become Invalid only with accepted derived structs. |

## Relationships and handoffs

Envelope-tree removal enables typed-mutation authority but does not prove
payload preservation or fallback parity. Payload normalization and page digest
preservation share a timing boundary: hash first, then convert. Ownership and
mutation share Arc shells but test different failures. The fallback reachability
record prevents vacuity in fallback safety. No dominance is claimed.

| Slug | Named handoff |
| --- | --- |
| envelope-decode-has-no-retained-tree | `/testing:test-strategy`: final source/representation witness; accounting agent: allocation evidence. |
| typed-failure-preserves-tree-outcome | `/testing:test-strategy`: frozen literal corpus, raw-token baseline/final comparison, and null-profile preservation; implementation owner: stop-and-report any raw-token acceptance change; `/testing:invariant-test-review`: A2 differential adequacy. |
| unknown-envelope-fields-do-not-erase-payload-values | `/testing:test-strategy`: placement-sensitive payload oracle; identity agent: accepted changed unknown-field byte basis; specification/integration owners: preserve R3 and record TE08 reconciliation before integration. |
| page-and-other-routes-retain-tree-semantics | `/testing:test-strategy`: raw-page digest vectors and generated-corpus seam; `/testing:invariant-test-review`: page parity checks. |
| decoded-snapshots-own-and-share-prefixes | `/testing:test-strategy`: mobility, owner-drop, and prefix sequence; issue 524 owner: production private-input release. |
| typed-mutation-is-visible-and-copy-isolated | `/testing:test-strategy`: independently constructed typed mutation matrix; `/testing:invariant-test-review`: contradictory stale-edit golden. |
| nested-duplicate-fallback-is-exercised | `/testing:test-strategy`: two independently required non-vacuous marker observations and an aggregate report that preserves each result; `/testing:invariant-test-review`: coverage oracle adequacy. |

Runtime guards in the inventory route to
`/low-level-systems:defensive-assertions-and-invariant-guards`. No seeded
distributed harness is justified by these local sequences. If test strategy
finds a schedule-dependent gap, it can hand that specific property to
`/testing:deterministic-simulation-testing`. The completed independent
portfolio pass and its applied refinements are recorded in
[portfolio-evaluation.md](portfolio-evaluation.md). Execution and the owned
TE08 integration reconciliation remain open.

## Initial mechanical verification receipt: 2026-09-13

The counts in this initial receipt are superseded by the post-evaluation
receipt below; they describe the discovery artifact before review edits.

The initial read-only validation confirms seven records, seven matching
index rows, seven matching evidence files, and the exact METHOD field order.
Evidence files contain 85-98 lines and include investigation logs and named
handoffs. All 23 lens files remain present. The semantics distribution is six
`always` checks and one `sometimes` check.

All 40 local Markdown links and anchors resolve. The verification reopens HEAD
sources for 107 fully qualified source references and separately checks 97
inventory locations, including named test/function anchors. This mechanical
range/name check complements the source inspection; it is not a semantic
proof or test-adequacy review. Whitespace checks pass. The accepted plan's
worktree hash still matches the provenance entry.

The initial 34 created files are confined to this decode directory. Existing
check status remains unaudited. No test, build, benchmark, tracker mutation, or
source edit is part of this receipt. Independent portfolio evaluation follows
this initial mechanical check on the same date, as recorded above; its
completion does not exercise any claim.

## Post-evaluation mechanical verification: 2026-09-13

The applied independent-review dispositions preserve the exact METHOD field
order and seven matching records, index rows, and evidence files. All seven
records remain active and unexercised. The 34-file decode part retains all
23 discovery lens reports; evidence files contain 87-109 lines.

All 44 local links and anchors resolve, 107 fully qualified source references
remain within their HEAD files, and whitespace checks pass. The 99 inventory
check entries remain unaudited. The semantics distribution remains six
`always` records and one `sometimes` record with two independently required
markers. The accepted plan hash is unchanged.

The four-lens evaluation and its disposition are complete under independent
analyst `ses_f6756093fffeVjNp36S3E8pKrM`. Raw-token execution evidence and TE08
integration reconciliation remain open. These mechanical checks do not run
the proposed behavioral tests or authorize specification publication.

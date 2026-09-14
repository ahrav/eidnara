# Typed wire decode: identity properties

## Scope and provenance

Date: 2026-09-13. System: CK identity and served serialization in
`crates/memory-store`, `crates/daemon`, and the OpenCode plugin emitter.
Source HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Frozen comparison baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.

This is the identity discovery input to the requested single `/to-spec` from
[the settled plan](../../../plans/2026-09-13-0104-perf-typed-wire-decode-plan.md).
It is not a published specification or implementation. All writes belong to
this directory. No tests, benchmarks, tracker mutations, or commits occurred.

The user supplied the external scope instead of an interview: the plan,
[normative host protocol](../../../host-wire-protocol.md), latency-audit
catalogs, and referenced issues as leads. No other incidents or external
repositories were supplied. Missing evidence remains recorded below.

Sources consulted and why:

- The plan, especially R3-R7, KTD2, mutation/receipt design, U2, and Appendix
  A.3, defines prospective compatibility promises and accepted exceptions.
- The host protocol freezes host names and literals. Sections 7.1 and 7.5.1
  do not specify CK application-body identity; line 763 leaves routed bodies
  to handlers. It remains normative over the plan for host wire behavior.
- [Latency-audit catalog](../../hot-path-optimization/latency-audit/catalog.md):
  A1/A2/A3 own admission and entry equivalence; B1 owns shared-input artifacts;
  B2/B5 own synthetic normalization; W5 owns history_summarizer construction. W1 is
  invalidated at HEAD and retained only for traceability; this catalog neither
  assigns it measurement ownership nor reactivates it. Their exercise status
  is historical evidence owned there.
  No such status is copied into this catalog.
- GitHub issues [350](https://github.com/ahrav/eidnara/issues/350),
  [426](https://github.com/ahrav/eidnara/issues/426),
  [435](https://github.com/ahrav/eidnara/issues/435),
  [436](https://github.com/ahrav/eidnara/issues/436),
  [438](https://github.com/ahrav/eidnara/issues/438),
  [441](https://github.com/ahrav/eidnara/issues/441), and
  [524](https://github.com/ahrav/eidnara/issues/524) were read with comments.
  They supply frozen-byte, decode, egress, blocking, and ownership constraints,
  not independently confirmed bug mechanisms or completed-work evidence.
- Fresh independent evaluator `ses_f6756093fffeVjNp36S3E8pKrM` completed
  harness-fit, coverage-balance, implementability, and wildcard lenses. The
  user supplied its findings and a test-strategy correction. Their dispositions
  are recorded in [portfolio-evaluation.md](portfolio-evaluation.md). Findings
  are evidence to verify, not authority to weaken accepted plan constraints.

Core plan-listed identity sources, the plugin emitter, and host protocol have
no diff between baseline and HEAD. Dirty `crates/daemon/src/lib.rs` references
are taken from `git show HEAD:crates/daemon/src/lib.rs`. Local `main` was not
used or checked out. `colgrep` was the primary search; it lacks indexed code
units for several large files, so exact HEAD scans supplied missing evidence.

## System model and reachability

The emitter builds JSON, custom wire decoders retain originals, projection
builds canonical replay shells and flat hashes, served messages select receipts
and serialize canonical message bytes, and the sidecar matches native parts.
Ingress identities, served receipts, hygiene baselines, and lineage anchor
hashes persist separately. Process caches are recreated empty; durable rows are
not. All twelve system and eleven property
lenses are retained in [_lenses](_lenses/). Wildcards ran after their named
lenses. Nested-agent dispatch was denied by the harness depth limit, so these
are separate focused passes by one discoverer, not independent-agent evidence.

Three targeted discovery reports extend that model after the independent
review: durable hygiene, lineage anchor enforcement, and byte-derived policy
outcomes. Their source paths are also unchanged between baseline and HEAD.

Each record states its own reachability. `default-production` means the path
requires ordinary valid messages or stored history, not an opt-in debug mode;
it does not claim every message shape occurs on every turn. The ordinary plugin
sets `serve_native: true` in
`packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts:745`.
Situation markers are `test-only`: each has its own `sometimes` check. The
reachability record is a reporting rollup, not a combined runtime assertion.

## Definitions and compatibility domains

- **Plugin domain P:** well-formed, JSON-serialized CK blocks emitted by
  `packages/opencode-plugin/src/hooks/context/module-wire.ts:940-1093`,
  including tool flags absent or true. Arbitrary
  JSON inside retained `Value` fields is part of the domain. Explicit false
  at the tool-kind flag, unknown typed-envelope fields, and extra typed-only
  output variants are classified separately, not silently included in P.
- **C(b):** the prospective `served_json::canonical_block_bytes(b)`. It uses
  the existing `served_json::encode` mechanism. There is no such block entry
  point at HEAD.
- **R(b):** an independent canonical typed reference, using sorted JSON from
  `serde_json::to_vec(&serde_json::to_value(fully_typed_b))`, supplemented by
  frozen baseline plugin bytes and literal omission cases. Retained originals
  must be removed in the reference setup; replaying an old original is not R.
- **H(bytes):** SHA-256 of the raw UTF-8 bytes; hexadecimal receipts use the
  full lowercase digest. Do not serialize the text as a JSON string first.
- **E(a,b):** prospective equality of `(kind, provider_extras)`. Equality
  indexing uses `block_identity_digest`, which is not the served-byte hash.
- **S(b):** a typed block with only the `_eidnara_codec` provider namespace
  removed. Other provider namespaces and opaque JSON remain intact.

## Index

| Slug | Type | Reachability | Semantics |
| --- | --- | --- | --- |
| [plugin-block-canonical-identity-preserved](#plugin-block-canonical-identity-preserved) | safety | default-production | always |
| [served-default-omissions-are-bounded](#served-default-omissions-are-bounded) | safety | default-production | always |
| [block-byte-consumers-share-canonical-basis](#block-byte-consumers-share-canonical-basis) | safety | default-production | always |
| [typed-equality-governs-receipt-reuse](#typed-equality-governs-receipt-reuse) | safety | default-production | always |
| [historical-chunks-retain-readable-identity](#historical-chunks-retain-readable-identity) | safety | default-production | always-or-unreached |
| [durable-identity-domains-survive-cache-reset](#durable-identity-domains-survive-cache-reset) | safety | default-production | always |
| [durable-hygiene-baseline-preserves-content-identity](#durable-hygiene-baseline-preserves-content-identity) | safety | default-production | always |
| [durable-lineage-anchor-preserves-validation](#durable-lineage-anchor-preserves-validation) | safety | default-production | always-or-unreached |
| [block-byte-policy-outcomes-remain-stable](#block-byte-policy-outcomes-remain-stable) | safety | default-production | always |
| [sibling-mutation-preserves-untouched-bytes](#sibling-mutation-preserves-untouched-bytes) | safety | default-production | always |
| [identity-edge-states-are-exercised](#identity-edge-states-are-exercised) | reachability | test-only | sometimes |

### plugin-block-canonical-identity-preserved

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `wire_golden_projects_to_flat_blocks` passes on the owned typed model with zero byte/hash drift against the emitter-faithful golden frozen at `85accd89`; the golden was not regenerated.
Guarantee: Every accepted plugin-domain block preserves its canonical bytes and ingress identity through typed decode.
Check: `always` - For each P block, assert `C(b) == R(b) == baseline_bytes(b)`, `flat.bytes == C(b)`, and `flat.content_hash == H(baseline_bytes(b))`; unchanged message IDs and block order must yield the same ingress identity vector, because projection computes identity on every accepted block.
Fault/timing angle: Switching serialization before the canonical producer changes ordering or emits a previously absent default.
Required faults and enabling state: Freeze absent/true tool flags, all emitted kind/output shapes, optional signatures/filenames/arcs, nested provider JSON, escaping, and non-ASCII values before changing the implementation.
Confidence: high - [evidence](evidence/plugin-block-canonical-identity-preserved.md). The emitter, retained serializer, and hash path are source-verified; replacement preservation is a claim under test.
Existing check: `crates/daemon/src/transform.rs:14467-14490` compares projection records to a golden; `served_json.rs:196-253` checks scalar/key order; both unaudited, with full inventory in [existing-checks.md](existing-checks.md).
Impact: Established sessions reject identity drift or lose a reusable prefix despite unchanged plugin content.
Open questions:

- The current golden has no absent/true tool flags and no media. Which frozen emitter-derived entries fill every P partition? (partial: shape gaps identified)
- The Appendix A.3 corpus is not supplied as a checked-in raw artifact; its 53-block observation is not universal evidence.

### served-default-omissions-are-bounded

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `served_canonical_shell_bytes_and_segments_are_frozen` (transform.rs) pins served bytes for decoded, flagged, and edited shells: unknown envelope keys are gone, false `synthetic` is omitted, true is kept; `typed_only_blocks_canonicalize_by_field_selection_and_sorted_order` pins explicit-false `provider_executed` decoding to the omitted default. No persisted synthetic-pair replay ran.
Guarantee: Plugin-shaped messages and daemon-built typed messages change served bytes only by the plan's permitted default-field omissions.
Check: `always` - Compare new served bytes to an independent sorted baseline JSON edit that removes only false `meta.synthetic`, `meta.summary`, and `meta.errored`, plus false tool-kind `provider_executed` in daemon-built typed blocks; preserve `meta` itself and every other field/value, because the exception is a bounded byte contract, not arbitrary semantic equality.
Fault/timing angle: A default skip predicate removes true values, changes null/empty handling, or hides unrelated output drift during golden regeneration.
Required faults and enabling state: Plugin absent/true flags; typed false call/result pair; false/true metadata mixtures; persisted frozen pair reload; explicit-false ingress and unknown envelopes labeled as separate R3/KTD2 normalization cases.
Confidence: high - [evidence](evidence/served-default-omissions-are-bounded.md). HEAD defaults and producer omissions are verified; the promised new bytes remain prospective.
Existing check: `crates/daemon/src/transform.rs:13714-13793` pins served forms; `crates/daemon/src/injection.rs:794-831` checks same-input determinism and changed-input inequality, not literal or baseline-pinned todo bytes; all unaudited.
Impact: A legitimate omission masks a signature, payload, failure-kind, or provider-field change.
Open questions:

- The parent specification must replace the 25-byte claim with 26 bytes per removed false tool member including its comma, or identify a different exact byte boundary. (needs human input)
- Older frozen-byte promises include nonplugin fixtures and persisted synthetic messages. The parent specification must state the accepted upgrade exception explicitly. (needs human input)

### block-byte-consumers-share-canonical-basis

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `tests/block_bases_agree.rs` and `fresh_block_byte_consumers_call_the_canonical_producer` pass after replay removal; `decoded_block_fingerprint` no longer clears an envelope before hashing.
Guarantee: Projection, fresh served fingerprints, and decoded sidecar fingerprints use one canonical typed block-byte basis with only the sidecar's codec namespace excluded.
Check: `always` - Assert `flat.bytes == C(b) == R(b)`, `flat.content_hash == H(C(b))`, fresh nonreused served receipt equals `(hex(H(C(b))), C(b).len())`, and sidecar fingerprint equals `hex(H(C(S(b))))`; the sidecar equals the other hash only when S does not change b, because namespace exclusion is intentional.
Fault/timing angle: One consumer keeps declaration-order serialization, hashes a JSON string containing C, or strips all provider extras instead of one namespace.
Required faults and enabling state: Force fresh served fallback, then use blocks with no stamp, with two different codec stamps, and with a controlled noncodec payload change; include typed-only and plugin-domain blocks.
Confidence: high - [evidence](evidence/block-byte-consumers-share-canonical-basis.md). All three HEAD paths and the exact exclusion are source-verified; their unification is prospective.
Existing check: `crates/daemon/src/codec/sidecar.rs:520-557` checks fingerprint matching; `transform.rs:13942-13991` checks source shape and receipt-helper use; unaudited. No `block_bases_agree.rs` exists at HEAD.
Impact: Equal blocks are classified as changed or matched to incorrect native metadata.
Open questions:

- The proposed helper must define its serialization-error behavior while reusing the existing raw-byte hash seam; `stable_hash(Value::String(C))` is incorrect.
- Reused receipts, especially signed-zero pairs, deliberately obey the next record rather than the fresh-receipt equation.
- Old sidecar fingerprinting marks every block modified before serialization, so even normal plugin tool blocks with an absent false flag acquire an explicit false member in that old hash basis. The proposed omission changes their normalized sidecar fingerprints too. This conflicts with the plan's broad sidecar-preservation wording and needs an owner decision before implementation. (needs human input)
- Handler cache storage is process-local, but the schema can persist codec-stamped provider extras in raw or frozen rows. No evidence excludes such legacy rows; their native alignment outcome cannot be waived as a cold-cache miss. (needs human input)

### typed-equality-governs-receipt-reuse

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `receipt_reuse_is_separate_from_fresh_hashing` now asserts that candidates differing only in discarded envelope fields are equal typed values with equal `block_identity_digest`s, that the first equal candidate is selected, and that signed-zero reuse preserves served bytes; the digest covers `(kind, provider_extras)` only.
Guarantee: Receipt reuse follows typed equality, positional precedence, and first-candidate selection without changing canonical served bytes.
Check: `always` - Assert `E(a,b) => D(a)==D(b)` for the equality digest D; select the first projected candidate at the served index, otherwise the first candidate indexed by D, recheck E, reuse exactly that candidate's hash/length when equal, and otherwise use the fresh C receipt; assert served message bytes equal serialization without receipt reuse, because receipts are metadata and must not rewrite the payload.
Fault/timing angle: Removing `original` broadens equality; a positional mismatch must not fall through to another index, and a hash match must not bypass equality.
Required faults and enabling state: Typed-equal ingress and constructed blocks, unequal provider extras, duplicate candidates, missing positions, unequal positional candidates, shifted indexes, integer zero, and floating positive/negative zero in both candidate orders.
Confidence: high - [evidence](evidence/typed-equality-governs-receipt-reuse.md). The HEAD algorithm and signed-zero distinction are explicitly represented in source and assertions; the new equality field set is prospective.
Existing check: `crates/daemon/src/transform.rs:13797-13938` pins precedence, first match, and signed-zero fresh/reused differences; unaudited.
Impact: Wrong projected receipts are attributed to served blocks or deterministic replay changes with allocation/provenance.
Open questions:

- Parent wording should say `IdentityFormatter` aligns equality digests, not served bytes: equal signed zeros can retain a selected projection receipt whose hash and length differ from fresh output. (needs human input)
- No reverse law `D(a)==D(b) => E(a,b)` is promised; equality rechecks remain required.

### historical-chunks-retain-readable-identity

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no old-release raw-row fixture was recovered with the replacement decoder.
Guarantee: Valid stored old-release CK message arrays remain readable and unchanged plugin chunk items retain their production history_summarizer fingerprint.
Check: `always-or-unreached` - When an old raw row is supplied, assert recovery returns the independently listed in-range IDs, ordinals, and known typed fields; assemble items from nonsynthetic, nonsystem blocks in the selected range and assert each `(id, kind, UTF-8 byte_len)` plus the exact joined `id:kind:len` string equals the old plugin baseline, because history is optional in production but mandatory in this campaign's companion witness.
Fault/timing angle: Upgrade silently skips a no-longer-decodable row or changes a length used by a durable in-flight fingerprint.
Required faults and enabling state: Baseline-produced serialized arrays, valid stored ordinal bounds, old false-default fields, a plugin tool pair, and a synthetic todo pair present beside real eligible blocks; test-only manually included synthetic items are labeled separately.
Confidence: high - [evidence](evidence/historical-chunks-retain-readable-identity.md). Recovery's silent skip and production synthetic exclusion are source-verified; upgrade execution is missing.
Existing check: `crates/daemon/src/history_summarizer.rs:3925-3950` pins fingerprint format; `history_summarizer_chunk.rs:1076-1103,1773-1858` pins assembly; `lib.rs:18182` compares boundary construction; all unaudited.
Impact: Historical messages disappear from expansion or a valid pending history_summarizer firing fails its fingerprint check.
Open questions:

- Which production path, if any, includes daemon-built synthetic todo blocks in chunk snapshot items? The inspected builder excludes them, so the plan's example is not production evidence. (needs human input)
- An old in-flight binary-upgrade witness is missing. Existing W5 evidence explicitly does not cover that transition.

### durable-identity-domains-survive-cache-reset

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no preserved-store old/new run with empty process caches was performed.
Guarantee: Clearing process-local caches preserves plugin ingress identities and synthetic exclusion while retaining the distinction between durable served receipts and ephemeral memo state.
Check: `always` - For identical accepted P ingress and persisted metadata, compare warm and cold projection identities and served bytes, assert effective synthetic mids are absent from `identity_by_mid`, and assert stored plugin vectors remain baseline-equal; separately compare persisted served receipt IDs/hash/length to the expected new receipt vector, allowing only the declared typed-output basis changes rather than treating the old vector as empty.
Fault/timing angle: A cold process masks a changed durable hash as a harmless memo miss or lets synthetic replay enter ingress enforcement.
Required faults and enabling state: Persisted covered/frozen plugin identities, a stored frozen synthetic pair and nonempty served receipt vector, a delta carrying unflagged synthetic IDs, and explicit fresh construction of output/native/projection caches.
Confidence: high - [evidence](evidence/durable-identity-domains-survive-cache-reset.md). The separate persistent fields, ingress exclusion, cache initialization, and diagnostic comparison are source-verified; upgrade outcomes remain untested.
Existing check: `crates/daemon/src/transform.rs:13638,14672,14832,28295` covers diagnostic drift, enforcement, and cache equivalence; `lib.rs:25162` covers synthetic delta preparation; all unaudited.
Impact: Existing sessions reject valid history, misattribute provider-cache divergence, or persist synthetic blocks as user ingress.
Open questions:

- Plan KTD2's cache-only rationale omits durable `ModuleMeta.served_output_fingerprint`, which includes synthetic output. What exact first post-upgrade diagnostic change is accepted? (needs human input)
- Unknown/explicit-false legacy ingress can already have persisted identities; the plan accepts their new identity but supplies no migration behavior for a covered or frozen old row. (needs human input)

### durable-hygiene-baseline-preserves-content-identity

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no old/new decode campaign reloads a stored hygiene baseline.
Guarantee: Unchanged plugin-domain inputs preserve durable hygiene part identities, content signatures, and refresh decisions across typed decode and memo reset.
Check: `always` - With identical core, coverage, tags, protection, prior baseline, clock, and bust flag, compare old/new full part tuples and measurement `(u,t,content_signature)`; for a block-byte-derived excluded part assert `content_hash == hex(H(UTF8("excluded\0") || baseline_block_bytes))`, signature equals the hash of ordered `key:content_hash\0` entries, and refreshed baseline fields equal the frozen old result, because zero-token parts still participate in durable prefix identity.
Fault/timing angle: A serializer change moves an excluded part's hash while token totals stay zero, invalidating a loaded baseline on a non-busting pass.
Required faults and enabling state: A persisted nonempty evaluable baseline containing an excluded plugin block; cold and warm memos; unchanged and append-only replay; a controlled content mutation; daemon-built and legacy unknown-envelope cases classified separately.
Confidence: high - [evidence](evidence/durable-hygiene-baseline-preserves-content-identity.md). The stored schema, kind-prefixed hashing, ordered signature, and prefix invalidation are verified at HEAD; preservation remains a proposed check.
Existing check: `crates/daemon/src/tail_hygiene.rs:1217,1265,2027,2071,2290` covers digest domains, memo behavior, refresh, and parity; all unaudited, with no old/new persisted-baseline witness.
Impact: Unchanged input becomes unevaluable and changes reminder-channel behavior despite unchanged U/T totals.
Open questions:

- Which outcome is required when daemon-built block ordering or false omission changes an excluded part hash in a persisted baseline? No semantic invalidation exception is granted by the plan. (needs human input)
- Unknown-envelope legacy baselines can retain hashes of discarded fields. Their upgrade handling must be reconciled with frozen behavior before implementation. (needs human input)

### durable-lineage-anchor-preserves-validation

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no replacement decoder has replayed a baseline-persisted anchor.
Guarantee: An unchanged plugin-domain lineage anchor preserves its durable hash and validation outcome while actual anchor violations retain fail-closed behavior.
Check: `always-or-unreached` - For a supplied baseline-valid unchanged P anchor, assert its `(anchor_block_id,anchor_content_hash,ordinal_continuation_base)` is unchanged, the matching flat hash equals the stored lowercase digest, and validation still succeeds under identical live order and text position; separately constructed missing identity, wrong ordinal, wrong first-live/last-text position, or real content mutation must retain the baseline validation error and production Defer/reconcile/no-trim path, because lineage state is optional but authoritative when present.
Fault/timing angle: Upgrade recomputation changes an unchanged anchor hash, causing persistent fail-closed deferral; clearing the anchor to avoid that error would conceal the regression.
Required faults and enabling state: A stored completed-descent anchor and ordinal base, a valid first-live last-text summary, a synthetic head and untouched siblings, and separately constructed missing/moved/mutated anchor controls.
Confidence: high - [evidence](evidence/durable-lineage-anchor-preserves-validation.md). Anchor creation, durable assignment, exact validation, and fail-closed consumption are source-verified; upgrade preservation is unexercised.
Existing check: `crates/daemon/src/transform.rs:28739,28914,29063,29121` and `crates/memory-store/src/lib.rs:26125` cover anchor/lineage behavior and persisted copy; all unaudited.
Impact: Valid continued sessions defer and lose trimming after upgrade, or genuine anchor corruption escapes enforcement.
Open questions:

- An old anchor hash can include unknown envelope fields that R3 drops. The plan gives no migration or accepted enforcement change for that durable identifier. (needs human input)
- Broad frozen guarantees and normalization must be reconciled by the owner; reviewer recommendations cannot authorize rewriting stored anchors. (needs human input)

### block-byte-policy-outcomes-remain-stable

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no frozen plugin outcome corpus compares old/new byte consumers.
Guarantee: Unchanged plugin-domain projection and policy inputs preserve byte-derived token counts, fold selection, and protected-tail outcomes across typed decode.
Check: `always` - Freeze old per-block UTF-8 lengths and estimator results, boundary token totals/prefixes, complete `BoundaryResolution` and `TriggerDecision`, selection decisions and outcome flags/counts, protected-tail floor ordinal, and any newly stored publication floor; compare every field after replacement with identical context, tags, core, clocks, and estimator in cold/warm paths, because equal hashes alone do not establish numeric or threshold behavior.
Fault/timing angle: Changed canonical lengths or tokenization cross an emergency reclaim, fold, or protected-tail threshold even though typed payloads compare equal.
Required faults and enabling state: Plugin text/tool/media/opaque projections near byte-rounding, token-suffix, trigger-budget, and emergency-rearm boundaries; untagged and tagged items; fixed estimator/policy inputs; separately observed daemon-only byte changes without approving their semantic effects.
Confidence: high - [evidence](evidence/block-byte-policy-outcomes-remain-stable.md). Boundary construction, original-token indexes, selection byte estimates, and publication-floor consumers are verified at HEAD; before/after outcomes are not measured.
Existing check: `crates/daemon/src/lib.rs:18182`, `crates/daemon/src/boundary.rs:2073,2117,2670,2736`, and `crates/daemon/tests/selection_differential.rs:2399` are related unaudited checks, not a replacement-decode outcome corpus.
Impact: A decode optimization changes what is folded, dropped, or protected on an otherwise unchanged plugin turn.
Open questions:

- Are daemon-only byte-basis changes observable at these policy consumers? Construct the real path and freeze its outcome; no permission to change token/fold/protection semantics follows from accepting a byte omission. (needs human input)
- If a daemon-only delta changes a branch, the owner must resolve the conflict with frozen behavior before implementation rather than rebaseline the outcome. (needs human input)

### sibling-mutation-preserves-untouched-bytes

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `a_block_edit_leaves_its_sibling_unchanged_and_envelope_unknowns_are_discarded` (memory-store) and `overlay_canonicalizes_only_the_mutated_block` (transform.rs) test the typed contract: an edit re-encodes one block, the untouched sibling is equal by value and keeps its payload, and unknown envelope keys are discarded on both.
Guarantee: Editing one typed block leaves all unedited sibling bytes and the shared source message unchanged.
Check: `always` - Snapshot C for every block after accepted ingress normalization, edit one selected block in an owned clone, assert every other sibling's C and hash are unchanged, the edited block equals its explicit expected bytes, and the aliased original retains all pre-edit bytes; for P siblings also compare against frozen baseline bytes, because the isolation promise applies to each mutation.
Fault/timing angle: Dropping message-level originals or rebuilding a clone accidentally changes sibling defaults, provider extras, signatures, or shared data.
Required faults and enabling state: A shared source, at least two blocks, one actual content mutation, a nontrivial untouched sibling, and fresh/full/reattached/incremental projections over the same effective input.
Confidence: high - [evidence](evidence/sibling-mutation-preserves-untouched-bytes.md). Accessor behavior and legacy assertions are verified; deterministic typed serialization is a prospective replacement mechanism.
Existing check: `crates/memory-store/src/lib.rs:16096-16112` and `crates/daemon/src/transform.rs:12733-12773` check sibling behavior under original replay; `wire.rs:1749` checks shared shells; all unaudited.
Impact: An unrelated block edit changes cache identity, provider bytes, or a still-shared request.
Open questions:

- Replace the legacy unknown-envelope sentinel with a retained typed payload sentinel while separately asserting R3 drops unknown envelopes; the mutation oracle must not demand preservation of an explicitly removed field.

### identity-edge-states-are-exercised

Type: reachability
Reachability: test-only
Status: active
Exercised: not yet - the independent situation markers and completed identity campaign do not exist.
Guarantee: An identity campaign independently constructs every required plugin, typed-only, durable-state, byte-policy, receipt, and mutation situation at least once.
Check: `sometimes` - Evaluate each of the twelve fixed input predicates in [fault-map.md](fault-map.md#independent-situation-markers) as its own named campaign assertion; this record reports their individual results without an aggregate runtime predicate, because one observed situation must not mask another unconstructed situation.
Fault/timing angle: A green suite skips the unsupported-looking shape, never forces fresh fallback, or omits preserved durable state during a cold-cache run.
Required faults and enabling state: The twelve independent predicates in the fault map, including all seven typed BlockKind and OutputKind variants, optional/null forms, stored hygiene/anchor state, and numeric threshold inputs.
Confidence: medium - [evidence](evidence/identity-edge-states-are-exercised.md). Existing constructors and test seams support the proposed situations, but corpus completeness and old/new execution are not demonstrated.
Existing check: None for this twelve-marker contract; existing checks and historical catalog evidence remain unaudited rather than satisfying it by association.
Impact: Preservation assertions pass vacuously or cover only the current sixteen-block mixed-domain golden.
Open questions:

- The completed fresh evaluation identifies missing typed-only and durable domains; test strategy must implement and observe each separate marker before claiming exercise.

## Conflicts and verified corrections

1. The removed member text is 25 bytes, but a nonempty tool object also loses
   a comma: the exact compact block delta is **26 bytes** per false flag.
   A synthetic pair loses 52 block bytes from that omission alone. This is
   neither universal over all blocks nor the Appendix's 26-of-53 corpus count.
2. Production history_summarizer snapshot assembly excludes synthetic blocks. A todo
   pair beside real history changes zero production snapshot entries merely
   because its own flags are omitted. A manually fabricated snapshot item is
   a test-only counterfactual, not evidence of the production path.
3. Signed-zero equality is broader than byte equality. Existing tests expressly
   preserve that distinction. The fresh-basis equation excludes reused receipts.
4. Durable served receipts and frozen todo messages contradict a blanket
   cache-only rationale. Synthetic ingress identity exclusion remains true.
5. The frozen projection fixture contains ten explicit-false tool blocks.
   Preserving every old fixture byte conflicts with accepted false omission;
   freeze actual plugin shapes and classify normalization cases explicitly.
6. `sidecar::stable_hash` hashes serialized `Value`; wrapping canonical text in
    `Value::String` would double-encode it. Raw-byte hashing already exists.
7. Sidecar normalization at HEAD adds false to normal absent-flag plugin tools
   before hashing. The proposed skip changes this normalized hash basis by 26
   bytes too, while their ingress bytes stay stable. Persisted stamped rows
   are representable and cannot be ruled out by a local cache's lifetime.
8. `tail_hygiene_baseline` and `anchor_content_hash` are additional durable
   hash domains. Zero-token excluded parts can invalidate a hygiene baseline;
   anchor mismatch drives Defer/reconcile/no-trim, not a harmless cache miss.
9. Byte-derived token/fold/protection outcomes have no new exception. Freeze
   plugin outcomes and escalate any daemon-only semantic effect to the owner.
10. W1 is invalidated at HEAD. Plan references to it do not reactivate it or
    transfer measurement ownership into this catalog.

Source-line correction: the plan names sidecar fingerprint lines 165-171;
the function is at `crates/daemon/src/codec/sidecar.rs:168-174` at HEAD.
All evidence references use checked HEAD locations rather than copied plan
offsets. These are source findings, not a claim that the replacement failed.

## Relationship map

Plugin byte preservation constrains the common producer, but common-producer
agreement alone can agree on the wrong bytes. The omission record defines the
permitted comparison boundary. Receipt equality is a different hash domain.
Historical and durable-domain records consume plugin preservation but also
need independent row/filter/cache evidence. Sibling isolation covers mutation
after normalization; it does not restore dropped unknown envelope fields.
The durable hygiene and anchor records remain separate from served receipts.
Numeric policy outcomes depend on exact bytes and fixed policy inputs, not
only on hash agreement. The reachability rollup supports all ten safety
records through independent markers and dominates none.

The existing latency-audit B1 and W5 records overlap several checks. These
records refine this plan's compatibility partitions, not replace those records
or inherit their exercised status. A2 owns decode outcomes and duplicate-key
fallback. W1 is invalidated and is not reactivated here. No liveness claim is added without a
bounded progress contract.

## Named handoffs

| Record | Test-form and oracle owner | Additional owner and seam |
| --- | --- | --- |
| plugin-block-canonical-identity-preserved | `/testing:test-strategy` | Specification author freezes P corpus; `/testing:invariant-test-review` audits projection and scalar goldens. |
| served-default-omissions-are-bounded | `/testing:test-strategy` | Specification author resolves the byte-delta and frozen-pair exception; independent byte-edit oracle owns expectations. |
| block-byte-consumers-share-canonical-basis | `/testing:test-strategy` | Identity implementer owns the `served_json` block entry and raw-byte hashing; sidecar owner preserves one-namespace exclusion. |
| typed-equality-governs-receipt-reuse | `/testing:test-strategy` | `/testing:invariant-test-review` audits signed-zero, candidate-order, and forced-fallback cases. |
| historical-chunks-retain-readable-identity | `/testing:test-strategy` | HistorySummarizer owner supplies baseline raw rows and selected snapshot inputs; `/testing:deterministic-simulation-testing` only if a restart-in-flight claim is added. |
| durable-identity-domains-survive-cache-reset | `/testing:test-strategy` | Specification author settles durable diagnostic drift and legacy ingress policy; `/low-level-systems:defensive-assertions-and-invariant-guards` audits enforcement. |
| durable-hygiene-baseline-preserves-content-identity | `/testing:test-strategy` | Hygiene owner freezes persisted baseline/refresh outcomes; invariant-test and invariant-guard reviewers audit zero-token hash invalidation. |
| durable-lineage-anchor-preserves-validation | `/testing:test-strategy` | Lineage owner supplies old anchors and fail-closed controls; specification owner resolves legacy unknown-field anchor hashes before implementation. |
| block-byte-policy-outcomes-remain-stable | `/testing:test-strategy` | Boundary/selection owner freezes numeric outcomes; specification owner decides any daemon-only conflict without silently granting semantic drift. |
| sibling-mutation-preserves-untouched-bytes | `/testing:test-strategy` | `/testing:invariant-test-review` separates legacy unknown-field replay from typed sibling isolation. |
| identity-edge-states-are-exercised | `/testing:test-strategy` | Implements twelve independent sometimes checks from the completed portfolio findings; the record only rolls up their status. |

## Completion state

Discovery records, check inventory, fault map, relationships, and evidence are
written. Semantics distribution: eight `always`, two `always-or-unreached`,
one `sometimes` rollup record; ten safety and one reachability record. The
rollup describes twelve separate named `sometimes` checks. Every record is
active and unexercised. The [portfolio file](portfolio-evaluation.md) records
the completed independent evaluation and dispositions. Owner decisions and
implementation evidence remain open; no new evaluator run is claimed.

Mechanical verification checks ordered METHOD records, matching index/evidence
sets, evidence lengths, preserved lens reports, local links and anchors, and
source references against HEAD. These are artifact checks, not executions of
the proposed properties or reactivation of invalidated W1.

Verified artifact totals: 41 Markdown files, eleven records and evidence files,
26 retained lens reports, and twelve independent situation assertions. Evidence
files remain within METHOD's 60-120-line range. All 251 inventory test names
match their stated HEAD line, and schema, links, marker uniqueness, and
index/evidence/handoff correspondence pass mechanical checks.

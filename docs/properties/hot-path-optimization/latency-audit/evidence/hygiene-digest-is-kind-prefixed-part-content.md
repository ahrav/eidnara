# hygiene-digest-is-kind-prefixed-part-content

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

Every `FlatBlock` already carries a SHA-256 `content_hash`, and the hygiene
measurement computes a second SHA-256 per part. The audit treats the second
hash as a candidate for removal. The two digests hash different inputs: the
projection hashes the full serialized `WireBlock`; hygiene hashes the part
kind name, a NUL, and a derived content string. The hygiene digest is both the
reported `content_hash` on each part and the token cache key, so substituting
the projection digest changes every reported hash and mixes cache entries.

## Evidence trail

- [`part_measurement`][part-measure] maps the kind to `text`, `toolInput`,
  `toolOutput`, `file`, or `excluded`, builds `kind_name ++ "\0" ++ content`,
  hashes it with SHA-256, reports `content_hash` as the lowercase hex of that
  digest, and for `PartTokens::Bpe` calls
  [`count_with_digest(digest, content)`][th-cwd].
- [`count_with_digest`][count-digest] looks up the digest in the current and
  previous generations before tokenizing; its doc says callers must hash a
  domain-separated injective encoding and names the hygiene encoding.
- [`measure_tail_hygiene`][hygiene] derives the content per block kind: for
  `Text` the caveman-substituted and reminder-stripped text
  ([text branch][hyg-text]); for `ToolCall` `serde_json::to_string(input)`
  ([input branch][hyg-input]); for `ToolResult` the reminder-stripped
  [`tool_output_content`][hyg-output] ([result branch][hyg-result]); for `Media`
  the media content ([media branch][hyg-media]).
- Exclusion inputs differ by branch. Blocks excluded before the kind match
  (synthetic, system, covered, reduced or sentinel arcs, red targets) hash
  `block.bytes` ([context exclusions][hyg-excluded]), as do reasoning, opaque,
  and system text blocks ([kind exclusions][hyg-excluded-kind]). A text, tool result,
  or media part whose derived content is empty or a drop sentinel is excluded
  with that derived content, not `block.bytes` ([text][hyg-text-empty],
  [result][hyg-result-empty], [media][hyg-media-empty]).
  [`excluded_part`][excluded-part] passes the string to `part_measurement`
  under `Excluded` with `PartTokens::Zero`.
- The projection digest is `sha256(to_string(block))` in
  [`flatten_block`][flatten]; it keys the boundary
  [`token_count`][token-count] cache, which counts `block.bytes`, a different
  input and a different cache.
- The existing checks are [`measurement_is_identical_with_cold_and_warm_token_cache`][t-hyg-cold]
  and [`parity_golden_matches_ts_reference_across_full_corpus`][t-hyg-golden],
  The TypeScript golden fixes U, T, and band, not per-part hashes. The Rust
  characterization below pins reported pre-memo full measurement output. Its
  execution provenance is agent-witnessed and transcript-only.

## Failure scenario

A shortcut reports `FlatBlock.content_hash` as the part's `content_hash`. Every
reported hash changes because the input is the serialized block rather than
the kind-prefixed derived text; the exact characterization fails. If the shortcut also
keys `count_with_digest` on the projection digest, a cached count of the
serialized block is returned for its text, and the cold and warm measurements
diverge because the first run tokenizes the text while the second returns the
other count.

## Timing windows and dependencies

The process pool hashes `(store namespace, session ID)` into sixteen fixed
mutex slots using a pool-owned hash seed. Each slot holds one optional session.
The lock spans measurement; the memo cannot be checked out or shared through
an `Arc`. Same-session calls serialize. Noncolliding slots can measure
concurrently. Colliding sessions serialize on the same slot and evict its
occupant, causing cold misses without changing measurement semantics.

Namespace-aware removal locks the selected slot and checks both identity
fields. Route invalidation, whose caller names only a session, removes that
ID across namespaces. Full reset and ID-only removal visit slots in index
order, releasing each lock before taking the next. These operations are not
an atomic pool snapshot: concurrent fills can repopulate a cleared slot.
Every hit still checks the caller's source and caveman state, so repopulation
does not authorize stale results. No borrowed memo escapes its slot lock.

A poisoned slot is purged while its guard is held, then `clear_poison` runs.
Recovery never trusts accounting that a panic may have interrupted. This
recovers cache reuse after unwinding; it does not recover an aborted process
or turn a panicking measurement into a successful response.
W3 owns token-cache key-domain non-aliasing. The TypeScript golden fixes
aggregate behavior, while the independent digest formula and pre-memo Rust
characterization fix the hash input and exact measurement output.

## What a test must construct

A tail with a text block, a tool call, tool results in text and content
variants, a media block, an excluded reduced block, a caveman-substituted text
block, and an empty or sentinel text part; measure the same input twice with a
cold and a warm token cache. For each part assert
`content_hash == hex(sha256(kind_name ++ "\0" ++ content))` with `content`
taken from the branch above, assert the cache lookup uses that digest, and
assert the two measurements are identical. The
[shared-input checks](../existing-checks.md#shared-input-equivalence) include
the cold-versus-warm identity, parity golden, digest-domain assertions, and
memo invalidation and retention cases.

For the excluded parts, the expected `content` is `block.bytes` on the
pre-match branch and the kind-level branch, and the derived empty or
drop-sentinel content on the text, tool-result, and media branches. The live
anchors above identify each branch.

## Investigation log

### Q: Is "excluded parts hash `block.bytes`" exact for every exclusion?

- Sources examined: [`measure_tail_hygiene`][hygiene] at
  [context exclusions][hyg-excluded], [text sentinel][hyg-text-empty],
  [result sentinel][hyg-result-empty], [media sentinel][hyg-media-empty],
  [kind exclusions][hyg-excluded-kind]; [`excluded_part`][excluded-part].
- Findings: The pre-match exclusions and the kind-level exclusions hash
  `block.bytes`. The empty-or-sentinel exclusions hash the derived content
  string. Both are `"excluded\0" ++ <string>` through `part_measurement`, so
  the kind-prefix clause holds; the `block.bytes` clause holds for the
  exclusion set the catalog's required faults name (reduced block) and needs
  the derived-content qualifier for the empty and sentinel branches.
- Missing evidence: None; this is a refinement of the check's wording, not a
  contract disagreement.
- Conclusion: resolved with answer - the digest input for an excluded part is
  `block.bytes` on the pre-match and kind-level branches and the derived
  content on the empty-or-sentinel branches; a test must cover both.

### Q: Does the bounded memo preserve measurements and account for retention?

- Sources examined: [memo storage and admission][memo], the
  [measurement caller][caller], [resource declaration][declaration], and
  [benchmark ownership][bench]. Live anchors are checked on 2026-09-12.
- Findings: Each session memo maps block identity to its own kind-prefixed
  digest, kind, and tokens. The projection digest is only an invalidator.
  Full caveman-unit equality, contextual exclusion, and text-role eligibility
  also gate reuse. Tag attribution, protection, coverage, reduced eligibility,
  full result parts, and the content signature are recomputed each call.
  A borrowed caveman map is built once per walk. The first duplicate key wins,
  matching the reference scan. Lookup does not repeatedly scan all frozen
  units or hash caveman payloads. The renderer's `FrozenUnitIndex` also builds
  reduction and tail-message indexes, so hygiene uses only the map it needs.
- Retention: Sixteen hashed slots each cap the accounted retained session key
  and memo heap at 1 MiB after operations. This is not a peak-allocation bound:
  insertion can allocate before the budget check drops an over-budget map,
  and the measurement also owns temporary data and result allocations.
  The declaration adds
  those caps and `size_of::<OnceLock<HygieneMemos>>()`, which includes the pool
  hash seed, sixteen slot mutexes, and inline session/map headers. Charges
  include map buckets, control-group allowance,
  keys, digest strings, and every retained caveman string capacity. Map charges
  retain their high-water allocation estimate across partial deletions. Oversized
  entries bypass retention; a map exceeding its budget is dropped. Empty
  projections release maps. Occupant mismatch compares namespace and ID before
  reuse. Different namespaces may occupy different slots; returning to one
  namespace never selects another namespace's memo. No wire field or persisted
  schema changes. The resulting bound is 16 MiB plus fixed container storage.
  Test recomputation independently checks counters but uses the same
  capacity-to-bucket model as production. It does not measure actual allocator
  RSS or independently validate hashbrown internals.
- Characterization provenance: Commit
  `d487b5549458796df1820a3a799ffb4bfb7146fa` contains only the release-accessor
  prerequisite repair, not the memo or its characterization test patch. The
  implementer witnessed a test-only patch against that pre-memo source, then ran
  `cargo test --locked -p daemon --lib tail_hygiene::tests -- --test-threads=1`.
  The first run had nine passes and one deliberate `CHARACTERIZE` placeholder
  failure, reporting the corpus aggregate SHA-256
  `01a4b82d5f0ce2853388f8c5e4f81509ca4e8b0bef4cb98d964f24ffd3deb4bf`.
  Replacing the placeholder with that digest and rerunning the same command
  gave ten passes. Memo edits followed those runs. This is agent-witnessed,
  transcript-only provenance supplied by the implementation controller, not
  independently reexecuted evidence. The tool transcript holds the receipt;
  no separate pre-memo characterization log or binary was preserved. No
  artifact-hash verification is claimed for that characterization.
- Verification history: The initial memo implementation passed 13 hygiene
  tests, four unchanged `differential_goldens` tests, and the duplicate-tool-use
  belt and session-recomp reset tests. The fixed-slot revision passes 16
  hygiene tests, the four
  unchanged goldens, and session-recomp reset. Its isolated production-transform
  test observes hit/miss counts of 0/3, then 3/0 for unchanged input, then 2/1
  after one block edit. A channel barrier proves two noncolliding sessions
  overlap inside their locks. Collision, namespace A/B/A, panic recovery through
  use/removal/reset, and repeated oversized multiblock walks have explicit
  assertions. All sixteen slots are filled near budget and checked against
  recomputed string capacities, modeled bucket charges, and pool storage.
  Prune, reinsert, replacement, and reset recompute counters in tests, under
  the accounting-model limitation above.
  Daemon all-target/all-feature
  clippy, release all-feature library check, and scoped rustfmt checks pass.
  Checks remain unaudited.
- Payoff evidence: The implementation pass did not execute benchmarks; the
  subsequent [frozen local payoff run](tail-hygiene-payoff.md) is complete.
  The benchmark creates and primes the same memo owner used by measurement
  before its callback and timed loop. Per-call slot hashing, locking, lookup,
  validity, accounting, and full-result construction/drop remain timed. The
  empty-core, empty-tag cell does not exercise caveman invalidation or populated
  attribution, and U is zero.
- Missing evidence: No independently replayable pre-memo characterization
  artifact, allocator/RSS validation, production workload, concurrent-session
  timing, or cold-call timing is established here.
- Conclusion: The recorded correctness and retained-accounting checks pass.
  The fixed three-pair A/A and five-pair A/B evidence meets both predeclared
  ticket-local payoff conditions, with a 73.1659% reduction in warm-call time.
  Retention is justified for that local payoff, not as a general latency or
  merge-readiness claim. The controller reports all 14 recent local gates
  passed; logs remain at `/tmp/opencode/hygiene-memo-*.log`. This documentation
  pass inspects logs and receipts without rerunning tests or benchmarks.

[flatten]: ../../../../../crates/daemon/src/wire.rs#L731-L796
[token-count]: ../../../../../crates/daemon/src/lib.rs#L2035-L2059
[hyg-output]: ../../../../../crates/daemon/src/tail_hygiene.rs#L478-L497
[part-measure]: ../../../../../crates/daemon/src/tail_hygiene.rs#L505-L528
[th-cwd]: ../../../../../crates/daemon/src/tail_hygiene.rs#L520
[excluded-part]: ../../../../../crates/daemon/src/tail_hygiene.rs#L530-L532
[hygiene]: ../../../../../crates/daemon/src/tail_hygiene.rs#L722-L879
[hyg-excluded]: ../../../../../crates/daemon/src/tail_hygiene.rs#L767-L790
[hyg-text]: ../../../../../crates/daemon/src/tail_hygiene.rs#L792-L803
[hyg-text-empty]: ../../../../../crates/daemon/src/tail_hygiene.rs#L798-L799
[hyg-input]: ../../../../../crates/daemon/src/tail_hygiene.rs#L804-L807
[hyg-result]: ../../../../../crates/daemon/src/tail_hygiene.rs#L808-L820
[hyg-result-empty]: ../../../../../crates/daemon/src/tail_hygiene.rs#L811-L812
[hyg-media]: ../../../../../crates/daemon/src/tail_hygiene.rs#L821-L836
[hyg-media-empty]: ../../../../../crates/daemon/src/tail_hygiene.rs#L823-L824
[hyg-excluded-kind]: ../../../../../crates/daemon/src/tail_hygiene.rs#L837-L840
[t-hyg-cold]: ../../../../../crates/daemon/src/tail_hygiene.rs#L1054
[t-hyg-golden]: ../../../../../crates/daemon/src/tail_hygiene.rs#L2027
[count-digest]: ../../../../../crates/daemon/src/token_cache.rs#L103-L143
[memo]: ../../../../../crates/daemon/src/tail_hygiene.rs#L69-L328
[caller]: ../../../../../crates/daemon/src/transform.rs#L4699-L4713
[declaration]: ../../../../../crates/daemon/src/lib.rs#L2250-L2269
[bench]: ../../../../../crates/daemon/benches/hot_path.rs#L90-L131

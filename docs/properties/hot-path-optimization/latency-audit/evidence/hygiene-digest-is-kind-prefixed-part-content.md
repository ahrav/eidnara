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

The process pool keeps a table of at most sixteen `(store namespace, session
ID)` entries behind one table lock, which covers only lookup, insertion,
least-recently-used eviction, and removal. Each session owns its memo behind
its own lock inside a shared `Arc`, and that lock spans the measurement.
Same-session calls serialize. Distinct sessions never block each other. A
seventeenth session evicts the least recently used entry, causing cold misses
for that session without changing measurement semantics. An in-flight walk
keeps its evicted memo alive through the `Arc` until it returns; the table no
longer charges it.

Namespace-aware removal drops the entry matching both identity fields. Route
invalidation, whose caller names only a session, removes that ID across
namespaces. Full reset clears the table. These operations take the table lock
once and never wait on a walk. Every hit still checks the caller's source and
caveman state, so repopulation does not authorize stale results.

A poisoned session memo is replaced with an empty memo while its guard is
held, its charge is zeroed, then `clear_poison` runs. A poisoned table is
cleared the same way. Recovery never trusts accounting that a panic may have
interrupted. This recovers cache reuse after unwinding; it does not recover an
aborted process or turn a panicking measurement into a successful response.
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
  A length-prefixed SHA-256 fingerprint of the caveman unit, contextual
  exclusion, and text-role eligibility also gate reuse; one `is_text_role`
  predicate serves the memo and the measurement branch. Tag attribution,
  protection, coverage, reduced eligibility, full result parts, and the
  content signature are recomputed each call. A borrowed caveman map is built
  once per walk. The first duplicate key wins, matching the reference scan.
  Lookup does not repeatedly scan all frozen units. The renderer's
  `FrozenUnitIndex` also builds reduction and tail-message indexes, so hygiene
  uses only the map it needs.
- Retention: Each of at most sixteen sessions caps its accounted session key
  and memo heap at 1 MiB after operations. Admission charges the entry's key
  and digest capacities plus the bucket allocation the map will hold after the
  insert, using the same routine as the charge; an entry that would exceed the
  budget is refused and counted, and the admitted working set is kept. A
  refused block stays cold; the admitted prefix stays warm on later walks. The
  memo retains no caveman payload bytes. This is not a peak-allocation bound:
  the measurement also owns temporary data and result allocations. The
  declaration adds the per-session caps, each session's table row and shared
  memo allocation, and `size_of::<OnceLock<HygieneMemos>>()`. Charges include
  map buckets, control-group allowance, keys, and digest strings. Map charges
  retain their high-water allocation estimate across partial deletions. Empty
  projections release maps. Table lookup compares namespace and ID; returning
  to one namespace never selects another namespace's memo. No wire field or
  persisted schema changes. The resulting bound is 16 MiB plus fixed container
  and per-session allocation storage. The module status reports the table's
  charged bytes, session count, and refused-insert counter under
  `tail_hygiene_memo`. Test recomputation independently checks counters but
  uses the same capacity-to-bucket model as production. It does not measure
  actual allocator RSS or independently validate hashbrown internals.
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
  belt and session-recomp reset tests. The session-table revision passes 19
  hygiene tests, the four unchanged goldens, and session-recomp reset. Its
  isolated production-transform test observes hit/miss counts of 0/3, then 3/0
  for unchanged input, then 2/1 after one block edit, and asserts that the
  child process ran exactly one test. A channel barrier proves sixteen
  distinct sessions hold their memos concurrently and stay warm afterwards.
  Namespace A/B/A, least-recently-used eviction at the limit, panic recovery
  through use/removal/reset, over-budget walks that keep a warm prefix, and
  payload-size-independent retention have explicit assertions. All sixteen
  sessions are filled past budget and checked against recomputed string
  capacities, modeled bucket charges, table storage, and the status metrics.
  Prune, reinsert, replacement, and refusal recompute counters in tests, under
  the accounting-model limitation above.
  Daemon all-target/all-feature clippy and rustfmt checks pass.
  Checks remain unaudited.
- Historical payoff evidence: The implementation pass did not execute
  benchmarks; the subsequent [frozen local payoff run](tail-hygiene-payoff.md)
  is complete.
  The benchmark creates and primes the same memo owner used by measurement
  before its callback and timed loop. Per-call table lookup, locking,
  validity, accounting, and full-result construction/drop remain timed. The
  timed loop measures the fully warm path only; cold walks, edits, and
  refusals are not timed, and the 2,500-message cell is not reported. The
  empty-core, empty-tag cell does not exercise caveman invalidation or populated
  attribution, and U is zero.
- Missing evidence: No independently replayable pre-memo characterization
  artifact, allocator/RSS validation, production workload, concurrent-session
  timing, or cold-call timing is established here.
- Historical conclusion: The recorded correctness and retained-accounting checks pass.
  The fixed three-pair A/A and five-pair A/B evidence meets both predeclared
  ticket-local payoff conditions, with a 73.1659% reduction in warm-call time.
  Retention is justified for that local payoff, not as a general latency or
  merge-readiness claim. The controller reports all 14 recent local gates
  passed; logs remain at `/tmp/opencode/hygiene-memo-*.log`. This documentation
  pass inspects logs and receipts without rerunning tests or benchmarks.

### Q: Does the integrated decoded-ingress workload retain a local payoff?

- Sources examined: The [integrated payoff evidence](tail-hygiene-integrated-payoff.md)
  and its [compact manifest](tail-hygiene-integrated-payoff.json), the safe
  experiment's plan, results, A/A and A/B summaries, source identities,
  checksums, raw samples, and recorded build/process receipts.
- Findings: The new experiment compares archived parent `16542f5e` plus only
  the required six-addition/six-deletion release-accessor repair with candidate
  `05c33bf0`. Both use the same decoded-ingress helper and retain original JSON.
  Three A/A pairs establish a new guard before five A/B pairs run under the
  same fixed rule. All 16 processes are valid. The warm-call time reduction is
  72.5513%; every paired log gain exceeds the new A/A guard, and the mean
  exceeds twice the sample SD. Both 477-input source maps and both binaries
  still match their measurement hashes during this documentation pass.
- Verification provenance: The controller reports all 14 local gates passed
  on `05c33bf0`; `/tmp/opencode/memo-integrated-*.log` contains the recorded
  outputs. Empty gate logs do not independently establish exit status or
  source revision. No test, build, or benchmark runs for this docs-only update.
- Missing evidence: Production, concurrent-session, cold-call, total-turn or
  session latency, and allocator/RSS validation remain outside the experiment.
  The paired interval is conditional on the exact artifacts and host window.
  The pre-memo characterization remains agent-witnessed and transcript-only.
- Conclusion: resolved with answer - the new measurement closes the integrated
  workload's payoff gap for this local warm-call boundary. The historical
  73.1659% result remains valid only for its earlier fixture and artifacts; it
  is not reinterpreted as an integrated result. Only the new safe experiment
  bundle is read for this update; the older secret-bearing raw bundle is not.

[flatten]: ../../../../../crates/daemon/src/wire.rs#L731-L796
[token-count]: ../../../../../crates/daemon/src/lib.rs#L2041-L2065
[hyg-output]: ../../../../../crates/daemon/src/tail_hygiene.rs#L572-L591
[part-measure]: ../../../../../crates/daemon/src/tail_hygiene.rs#L599-L622
[th-cwd]: ../../../../../crates/daemon/src/tail_hygiene.rs#L614
[excluded-part]: ../../../../../crates/daemon/src/tail_hygiene.rs#L624-L626
[hygiene]: ../../../../../crates/daemon/src/tail_hygiene.rs#L816-L973
[hyg-excluded]: ../../../../../crates/daemon/src/tail_hygiene.rs#L861-L885
[hyg-text]: ../../../../../crates/daemon/src/tail_hygiene.rs#L888-L897
[hyg-text-empty]: ../../../../../crates/daemon/src/tail_hygiene.rs#L892-L893
[hyg-input]: ../../../../../crates/daemon/src/tail_hygiene.rs#L898-L901
[hyg-result]: ../../../../../crates/daemon/src/tail_hygiene.rs#L902-L914
[hyg-result-empty]: ../../../../../crates/daemon/src/tail_hygiene.rs#L905-L906
[hyg-media]: ../../../../../crates/daemon/src/tail_hygiene.rs#L915-L930
[hyg-media-empty]: ../../../../../crates/daemon/src/tail_hygiene.rs#L917-L918
[hyg-excluded-kind]: ../../../../../crates/daemon/src/tail_hygiene.rs#L931-L934
[t-hyg-cold]: ../../../../../crates/daemon/src/tail_hygiene.rs#L1148
[t-hyg-golden]: ../../../../../crates/daemon/src/tail_hygiene.rs#L2281
[count-digest]: ../../../../../crates/daemon/src/token_cache.rs#L103-L143
[memo]: ../../../../../crates/daemon/src/tail_hygiene.rs#L69-L417
[caller]: ../../../../../crates/daemon/src/transform.rs#L4699-L4713
[declaration]: ../../../../../crates/daemon/src/lib.rs#L2256-L2275
[bench]: ../../../../../crates/daemon/benches/hot_path.rs#L161-L199

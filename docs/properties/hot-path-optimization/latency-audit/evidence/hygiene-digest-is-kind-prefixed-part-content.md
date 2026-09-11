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
  ([`:536-554`][hyg-text]); for `ToolCall` `serde_json::to_string(input)`
  ([`:555-565`][hyg-input]); for `ToolResult` the reminder-stripped
  [`tool_output_content`][hyg-output] ([`:566-581`][hyg-result]); for `Media`
  the media content ([`:582-600`][hyg-media]).
- Exclusion inputs differ by branch. Blocks excluded before the kind match
  (synthetic, system, covered, reduced or sentinel arcs, red targets) hash
  `block.bytes` ([`:513-526`][hyg-excluded]), as do reasoning, opaque, and
  system text blocks ([`:601-604`][hyg-excluded-kind]). A text, tool result,
  or media part whose derived content is empty or a drop sentinel is excluded
  with that derived content, not `block.bytes` ([`:542-543`][hyg-text-empty],
  [`:569-570`][hyg-result-empty], [`:584-585`][hyg-media-empty]).
  [`excluded_part`][excluded-part] passes the string to `part_measurement`
  under `Excluded` with `PartTokens::Zero`.
- The projection digest is `sha256(to_string(block))` in
  [`flatten_block`][flatten]; it keys the boundary
  [`token_count`][token-count] cache, which counts `block.bytes`, a different
  input and a different cache.
- The existing checks are [`measurement_is_identical_with_cold_and_warm_token_cache`][t-hyg-cold]
  and [`parity_golden_matches_ts_reference_across_full_corpus`][t-hyg-golden],
  whose golden carries per-part hashes from the TypeScript reference.

## Failure scenario

A shortcut reports `FlatBlock.content_hash` as the part's `content_hash`. Every
reported hash changes because the input is the serialized block rather than
the kind-prefixed derived text; the parity golden fails. If the shortcut also
keys `count_with_digest` on the projection digest, a cached count of the
serialized block is returned for its text, and the cold and warm measurements
diverge because the first run tokenizes the text while the second returns the
other count.

## Timing windows and dependencies

None in time. The dependencies are the TypeScript golden, which fixes the hash
input externally, and W3, which owns the key-domain non-aliasing clause for the
token cache.

## What a test must construct

A tail with a text block, a tool call, tool results in text and content
variants, a media block, an excluded reduced block, a caveman-substituted text
block, and an empty or sentinel text part; measure the same input twice with a
cold and a warm token cache. For each part assert
`content_hash == hex(sha256(kind_name ++ "\0" ++ content))` with `content`
taken from the branch above, assert the cache lookup uses that digest, and
assert the two measurements are identical. The
[shared-input checks](../existing-checks.md#shared-input-equivalence) include
the cold-versus-warm identity and the parity golden; none states the digest
input against the projection digest.

For the excluded parts, the expected `content` is `block.bytes` on the
pre-match branch (`tail_hygiene.rs:513-526`) and the kind-level branch
(`:601-604`), and the derived empty or drop-sentinel content on the text,
tool-result, and media branches (`:542-543`, `:569-570`, `:584-585`, through
`excluded_part` at `:280-289`).

## Investigation log

### Q: Is "excluded parts hash `block.bytes`" exact for every exclusion?

- Sources examined: [`measure_tail_hygiene`][hygiene] at
  [`:513-526`][hyg-excluded], [`:542-543`][hyg-text-empty],
  [`:569-570`][hyg-result-empty], [`:584-585`][hyg-media-empty],
  [`:601-604`][hyg-excluded-kind]; [`excluded_part`][excluded-part].
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

[flatten]: ../../../../../crates/daemon/src/wire.rs#L622-L685
[token-count]: ../../../../../crates/daemon/src/lib.rs#L2028-L2050
[hyg-output]: ../../../../../crates/daemon/src/tail_hygiene.rs#L215-L234
[part-measure]: ../../../../../crates/daemon/src/tail_hygiene.rs#L242-L278
[th-cwd]: ../../../../../crates/daemon/src/tail_hygiene.rs#L264
[excluded-part]: ../../../../../crates/daemon/src/tail_hygiene.rs#L280-L289
[hygiene]: ../../../../../crates/daemon/src/tail_hygiene.rs#L472-L526
[hyg-excluded]: ../../../../../crates/daemon/src/tail_hygiene.rs#L513-L526
[hyg-text]: ../../../../../crates/daemon/src/tail_hygiene.rs#L536-L554
[hyg-text-empty]: ../../../../../crates/daemon/src/tail_hygiene.rs#L542-L543
[hyg-input]: ../../../../../crates/daemon/src/tail_hygiene.rs#L555-L565
[hyg-result]: ../../../../../crates/daemon/src/tail_hygiene.rs#L566-L581
[hyg-result-empty]: ../../../../../crates/daemon/src/tail_hygiene.rs#L569-L570
[hyg-media]: ../../../../../crates/daemon/src/tail_hygiene.rs#L582-L600
[hyg-media-empty]: ../../../../../crates/daemon/src/tail_hygiene.rs#L584-L585
[hyg-excluded-kind]: ../../../../../crates/daemon/src/tail_hygiene.rs#L601-L604
[t-hyg-cold]: ../../../../../crates/daemon/src/tail_hygiene.rs#L795
[t-hyg-golden]: ../../../../../crates/daemon/src/tail_hygiene.rs#L1104
[count-digest]: ../../../../../crates/daemon/src/token_cache.rs#L103-L143

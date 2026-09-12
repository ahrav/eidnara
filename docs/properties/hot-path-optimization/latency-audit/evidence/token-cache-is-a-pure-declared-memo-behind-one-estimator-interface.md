# token-cache-is-a-pure-declared-memo-behind-one-estimator-interface

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The token-count cache is the one process-global cache on the per-turn path,
and its source carries a conditional note proposing to shard it. The wildcard
pass asked two questions the H records do not: what makes a replacement cache
safe, and whether every decision-bearing token count inside a pass goes
through the interface a test can substitute. The second question surfaced
three direct `tokenizer::estimate_tokens` calls in production transform code.

## Evidence trail

- The [module doc][tc-doc] states the memo contract: `estimate_tokens` is
  pure, so a digest-keyed count reuses across passes and sessions without
  changing any rendered byte.
- The cache is one global [`Mutex<Option<Generations>>`][tc-static] over
  `current` and `previous` maps, rotated at
  [`GENERATION_CAP = 65_536`][tc-cap] by `insert_current`.
  [`RETAINED_BYTES_BOUND`][tc-bound] is derived from exactly two generations
  of that cap and is one term of
  [`DECLARED_RETAINED_RESIDENT_BYTES`][declared], whose [doc][declared-doc]
  says the bound holds only when the declaration is truthful and lists each
  retention class so a change cannot omit one.
- [`count_with_digest`][tc-cwd] bumps `calls`, returns a hit from either
  generation, tokenizes outside the lock, and returns counts above
  `u32::MAX` uncached ([`:135-137`][tc-u32]). Its doc says concurrent misses
  may tokenize the same content twice ([`:108-109`][tc-concurrent]). The
  sharding note at [`:112-113`][tc-shard] is conditional ("if concurrent
  sessions ever contend here") and cites no measurement.
- Key domains: tail hygiene hashes `kind_name ‖ NUL ‖ content` and calls
  `count_with_digest` at [`tail_hygiene.rs:614`][th-cwd];
  [`cached_estimate_tokens`][tc-cet] hashes `NUL ‖ content`, bypasses inputs
  under `MIN_CACHED_LEN = 64` with a `bypassed` bump, and the two domains are
  disjoint because no kind name can prefix a NUL.
- Counters are thread-local ([`:57-76`][tc-local]); `calls == hits + misses +
  bypassed` holds in a single reading.
- [`transform_with_projection_cached`][tc-inject] passes
  `cached_estimate_tokens` as the estimator on every production pass;
  [`apply_once`][ao-sig] takes it as `estimate_tokens: impl Fn(&str) -> usize
  + Copy`. The [comment][hard-only-doc] above the test-only wrapper says the
  estimator is HARD-only; the SOFT predicate's calls below are not covered
  by that statement.
- Direct `tokenizer::estimate_tokens` calls in production transform code:
  the SOFT predicate's `m0_tokens` and `m1_tokens` at
  [`:4306-4317`][soft-direct] (W9); the tag-mint `token_count` persisted into
  `TagMintInput` at [`:7160`][mint-direct]; `ActiveTagForNudge.token_count`
  at [`:8569`][nudge-direct]. The tokenizer crate's
  [`estimate_tokens`][tok-fn] exposes no call counter.
- The existing source scan
  [`protected_floor_has_no_global_estimator_bypass`][t-bypass] slices the
  source between `fn protected_tail_floor_ordinal(` and
  `fn post_end_revision_inputs_moved` and asserts the slice contains no
  direct call; it does not cover the three sites above.

## Failure scenario

A sharded replacement returns a different count for one input (a truncated
`u32`, a stale entry after a rotation race, or a key that aliases across
domains), so a budget decision or a persisted `token_count` changes while
every rendered byte still looks plausible. Or the replacement adds a shard
table without adding its bytes to the declaration, and the resident bound
undercounts. Or an optimization batches counts through a path outside the
injected estimator, so `tokenize_calls` no longer accounts for a
decision-bearing count and a test cannot substitute it.

## Timing windows and dependencies

Two sessions missing on one digest at the same time reach the double
tokenization the doc permits; a rotation at the cap while a promote-on-hit
insert runs is the other concurrent case. Both are latency-only under the
current code; a replacement must keep them value-neutral.

## What a test must construct

Two concurrent passes over shared content; 65_536 distinct digests to force
a rotation; a pass minting new tags; a SOFT pass whose predicate crosses a
threshold; and an assertion that `DECLARED_RETAINED_RESIDENT_BYTES` equals the
sum of its terms after the change. The
[wildcard checks](../existing-checks.md#wildcard-and-cross-cutting) list the
five token-cache tests and the one-helper source scan; none checks the
declared sum, measures contention, or scans the whole `apply_once` body.

## Investigation log

### Q: Is there measured lock contention at HEAD?

- Sources examined: [`:112-113`][tc-shard], the tokenize-outside-the-lock
  comment inside [`count_with_digest`][tc-cwd], the bench header in
  `crates/daemon/benches/hot_path.rs`.
- Findings: The note is conditional and the comment on tokenizing outside the
  lock gives a 2 KiB cost of about 80 us without a source; the bench is
  single-threaded by its own claim statement.
- Missing evidence: A multi-session measurement of lock hold time.
- Conclusion: unresolved, needs a contention measurement before sharding.

### Q: Should `:7160` and `:8558` stay direct or route through the interface?

- Sources examined: [`:7160`][mint-direct] in the tag-mint loop,
  [`:8569`][nudge-direct] in the nudge derivation.
- Findings: Both count `taggable_source` text of tail blocks and store the
  result in a durable or served `token_count`; neither is counted in
  `tokenize_calls`, and neither is reachable by an injected estimator.
- Missing evidence: A specification decision on accounting scope.
- Conclusion: needs human input.

### Q: Extend the source scan or replace it with a counting estimator?

- Sources examined: [`t-bypass`][t-bypass], [`ao-sig`][ao-sig].
- Findings: The scan is text-based and covers one helper; the injected
  estimator plus `tokenize_calls` is a runtime oracle for every call that
  goes through the parameter, and cannot see the three direct sites.
- Missing evidence: None for the mechanism; the choice is a test-strategy
  decision.
- Conclusion: unresolved, needs `/testing:test-strategy`.

## Implementation evidence

The preceding discovery snapshot is retained at its stated baseline. Current
checks and decision provenance are in [shared selection and pressure
accounting](shared-selection-and-pressure-accounting.md).

The accounting choice is resolved without adding parameters to tag helpers:
SOFT uses the injected estimator; tag minting and nudge derivation use the
existing cache entry point. The whole-production-module scan rejects direct
tokenizer paths and explicitly includes serialization, SOFT, minting, and
nudge helpers. It fails on all four direct baseline calls before edits.

Runtime checks cover exact SOFT inputs and classification, cached/direct
equality, placeholder and missing-m0 call gates, two warm hits, tag/nudge
counter deltas, and zero serialization estimates. The declaration and cache
capacity are unchanged. Contention measurement and an independent recomputed
declaration sum remain outside this change.

[tc-doc]: ../../../../../crates/daemon/src/token_cache.rs#L1-L7
[tc-cap]: ../../../../../crates/daemon/src/token_cache.rs#L16
[tc-bound]: ../../../../../crates/daemon/src/token_cache.rs#L24-L28
[tc-static]: ../../../../../crates/daemon/src/token_cache.rs#L34-L40
[tc-local]: ../../../../../crates/daemon/src/token_cache.rs#L57-L76
[tc-concurrent]: ../../../../../crates/daemon/src/token_cache.rs#L108-L109
[tc-cwd]: ../../../../../crates/daemon/src/token_cache.rs#L110-L142
[tc-shard]: ../../../../../crates/daemon/src/token_cache.rs#L112-L113
[tc-u32]: ../../../../../crates/daemon/src/token_cache.rs#L135-L137
[tc-cet]: ../../../../../crates/daemon/src/token_cache.rs#L165-L181
[th-cwd]: ../../../../../crates/daemon/src/tail_hygiene.rs#L614
[declared-doc]: ../../../../../crates/daemon/src/lib.rs#L2243-L2248
[declared]: ../../../../../crates/daemon/src/lib.rs#L2250-L2264
[tc-inject]: ../../../../../crates/daemon/src/transform.rs#L1810-L1826
[hard-only-doc]: ../../../../../crates/daemon/src/transform.rs#L1868-L1870
[ao-sig]: ../../../../../crates/daemon/src/transform.rs#L2847-L2856
[soft-direct]: ../../../../../crates/daemon/src/transform.rs#L4309-L4320
[mint-direct]: ../../../../../crates/daemon/src/transform.rs#L7163
[nudge-direct]: ../../../../../crates/daemon/src/transform.rs#L8572
[t-bypass]: ../../../../../crates/daemon/src/transform.rs#L24458-L24469
[tok-fn]: ../../../../../crates/tokenizer/src/lib.rs#L148

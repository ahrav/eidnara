# tag-baseline-cache-entry-is-never-mutated-by-a-pass

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

The discovery sections below preserve the baseline inspection. Their code
links are pinned to that commit. The shared-row execution evidence and live
anchors follow the historical investigation log.

## Discovery trigger

The audit notes that `Arc::make_mut(tag_rows)` copies the whole baseline
vector on every pass whose rows came from a cache hit, and proposes appending
in place or storing the pass's vector back into the cache. The copy exists
because the cache holds a second reference to the same `Arc`. Removing it
without another isolation mechanism exposes pass-local mint rows, which the
store has not numbered, to the next pass of the same session through the
cache. The transform catalog's [two-authorities record][tc-tagnum] covers the
numbering split; this record covers the cache entry's immutability and the
`source_bytes` identity.

## Evidence trail

- [`TagBaselineCacheEntry`][tag-entry] records `store_namespace`,
  `generation`, `count`, `max_tag_number`, and `tags: Arc<Vec<TagRow>>`;
  `matches` requires all four keys equal and `can_append` requires the
  summary to have grown by exactly the appended count.
- [`snapshot`][tag-snapshot] returns `self.sessions.get(session_id)?.clone()`,
  a clone of the entry and therefore a second `Arc` reference to `tags`.
- [`load_cached_tags`][load-tags] returns `entry.tags` on a match; on
  `can_append` it reads `load_tags_after`, re-reads the summary, and appends
  only when `observed == summary`, the tail length equals the appended count,
  and the last tail row carries `max_tag_number`; otherwise it reloads
  [`load_tags_for_session`][load-order], which orders by `tag_number ASC`.
- The pass loads `tag_rows` at [`:3002`][load-call] and later runs
  [`append_tag_mint_rows(Arc::make_mut(tag_rows), ..)`][make-mut] whether or
  not the mint batch is empty. [`append_tag_mint_rows`][append-mint] numbers
  each mint row `max + offset + 1` in the order of `tag_mints`, which is
  projection block order, and returns the start index.
- Mint inputs are built at [`:7156-7161`][mint-input] with
  `source_bytes: source.as_bytes().to_vec()` from [`taggable_source`][taggable],
  which returns the text of a user or assistant `Text` block or the first text
  of a tool result. The active-tag match at [`:7353`][active-match] compares
  `row.source_bytes == source.as_bytes()` for the same predicate.
- The commit takes mint inputs from `tag_rows[tag_mint_start..]`
  ([`:4924-4934`][commit-inputs]). The store numbers each new row from
  [`COALESCE(MAX(tag_number), 0) + 1`][store-number] inside the write
  transaction and skips existing block ids ([`:7140-7145`][store-existed]);
  `source_bytes` pass through
  [`write.bytes("source_bytes", ..)`][mint-prepared],
  where a detection refuses the insert unless the row existed.
- No code path in `transform.rs` stores the pass's `tag_rows` back into the
  cache; only `load_cached_tags` calls `replace`, with rows read from the store.

## Failure scenario

A design appends to the shared vector in place. The cache entry now holds mint
rows numbered by the pass, not the store. On the next pass `snapshot` returns
them; the active-tag match at [`:7353`][active-match] treats a speculative row
as active, and if the mint commit failed the row never existed. A design that
stores the pass's `Arc` back before commit has the same window. A design that
changes the `source_bytes` capture so it no longer equals the projected text
breaks the match for every row.

## Timing windows and dependencies

The exposure window is between `append_tag_mint_rows` and the commit's CAS
result; a failed CAS leaves the speculative rows uncommitted. The store side
has its own window: `load_cached_tags` appends `load_tags_after` only when the
summary is unchanged across two reads. The `source_bytes` identity depends on
the prepared-field path returning the bytes unchanged, which [R1][r1] owns.

## What a test must construct

Tagging active (a profile with `tool_present`), a warm baseline entry, a pass
that mints, then a second pass on the same session. For the aliasing arm, a
mint batch whose commit CAS fails so the speculative rows never land. After
each pass compare the entry's `tags` with `load_tags_for_session` for the
recorded `(store_namespace, generation, count, max_tag_number)`, check the
pass-local vector is baseline plus mint rows with `max + offset + 1`, and
compare every committed `source_bytes` with the block's `taggable_source`
text. The [shared-input checks](../existing-checks.md#shared-input-equivalence)
cover cold-versus-cached parity across five passes ([`t-tagcold`][t-tagcold]),
refill after a direct SQL update ([`t-poison`][t-poison]), and session
isolation ([`t-interleave`][t-interleave]); none fails a mint commit.

## Investigation log

### Q: Is the assumption that landed `source_bytes` are unchanged stated?

- Sources examined: [`write.bytes("source_bytes", ..)`][mint-prepared], the
  [mint input capture][mint-input], the parent's [R1][r1].
- Findings: The store either refuses the insert on a detection or, for an
  existing row, records the refusal and keeps the input bytes. No document
  states that a stored `source_bytes` equals its projected text; the active
  match at [`:7353`][active-match] depends on it.
- Missing evidence: A written statement in R1 or here.
- Conclusion: needs human input.

### Shared-row implementation and local verification

Execution provenance: 2026-09-11, working tree based on `9d04c24b`.

- The [cache entry][live-entry] holds `Arc<[Arc<TagRow>]>`. The pass owns a
  separate mint tail. [Overlay computation][live-tail] chains the baseline
  and tail while resolving mint times; the caller [combines handles][live-combined]
  only for a nonempty tail. The [standalone sharing test][live-sharing] checks
  row and source pointers in the combined view and hygiene output.
- [Hygiene][live-hygiene] filters and sorts shared row handles. Derived rows
  remain local. The [commit inputs][live-commit] come only from the mint tail,
  even when selection folds its carriers. Neither successful nor failed
  commits publish that tail into the baseline cache.
- Only [store refill][live-load] replaces the cache entry. An append refill
  copies existing row handles and reads committed additions; generation,
  count, maximum number, and the second summary read retain their guards.
- [Retention][live-charge] charges each row's capacities, inline row, pointer,
  Arc counters, and the slice Arc counters with saturating arithmetic. It
  retains the 64-byte per-row allowance and 64 MiB cap. Shared rows receive
  full charges, so sharing cannot discount admission. The [charge test][live-charge-test]
  uses spare capacities and repeated handles. This estimates retained bytes,
  not allocator RSS or memory retained by active passes.
- Before the implementation, `cargo test -p daemon --lib --locked tag_baseline`
  passed six tests with one manual timing test ignored. The `tag_mint` filter
  passed three tests. The added [append-refill pointer check][live-interleave]
  failed because the predecessor copied the baseline source allocation.
  The [mint/hygiene pointer test][live-sharing] also failed on the predecessor's
  pass copy. Both pass with shared rows; the checks retain the old owner so
  allocator address reuse cannot hide a copy.
- The [failed-commit test][live-rollback] passed before and after the change.
  The existing transform-attempt hook installs a temporary SQLite trigger
  that aborts the second mint insert. The first mint, cache-state write,
  generation, and temporal rows do not survive. The cache retains its exact
  baseline pointer and independently copied contents. A retry renders tags
  2 and 3; the cache stays at one row until explicit refill. Every refilled
  clean source equals the corresponding projected text, including Unicode,
  leading spaces, and a trailing newline.
- Focused filters `tag_baseline`, `tag_mint`,
  `mint_scope_matches_overlay_scope_and_captures_exact_source`, `pending`,
  `tail_hygiene`, and `differential_goldens` pass with
  `cargo test -p daemon --lib --locked`. The three integration targets
  `caveman_differential`, `selection_differential`, and
  `historian_truncate_differential` pass (24 tests).
- `cargo clippy -p daemon --all-targets --all-features --locked -- -D warnings`
  and scoped `rustfmt --edition 2024 --check` pass. A full daemon library run
  reports 1093 passed, two failed, and four ignored. Both failures are
  `dreamer_run_task_bounds_*` deadline tests in unchanged `lib.rs`; both pass
  when rerun together with `--test-threads=1`. The broad run is not a green gate.
- `differential_goldens.rs` is byte-identical to `9d04c24b`, Git blob
  `7a32a3236bd3102dc764cd36616edfd7920c736c`. No measurements, schema changes,
  storage API changes, or durability changes are part of this evidence.
  Full repository gates and independent reviews belong to the controller.

### Q: Does the transform commit reject every source-byte detection?

- Sources examined: [commit preparation][live-prepared],
  [`PreparedWrite::bytes`][live-bytes], and [field policy][live-policy].
- Findings: The historical trail cites the separate tag-mint API. The
  transform commit uses `tag_source_bytes` with the content policy, which
  substitutes detected secrets. The historical blanket refusal claim is not
  supported by this path. The pass still captures exact taggable bytes and
  the clean-text tests preserve them; storage policy is unchanged.
- Missing evidence: An agreed equality contract for redacted tag sources.
- Conclusion: needs human input. The record retains this open question and
  does not claim that clean-source tests prove equality for detected secrets.

### Shared-row follow-up verification

- The pass keeps the [immutable named baseline][live-baseline] separately
  from the combined view with one slice-Arc clone. [Bootstrap protection][live-protection]
  reads baseline rows directly, without a prefix-length convention.
  `PendingOverlayDecisions` owns the speculative tail. Standalone sharing
  tests and the rollback test's independently copied contents remain intact.
- [Hygiene measurement][live-measure] requires each iterator clone to yield
  the same row sequence independently. Callers should use constant-time
  clones of borrowed slices or iterator views. The transform passes
  `iter().map(Arc::as_ref)`; the benchmark adapter passes a borrowed slice.
  Neither needs a per-call `Vec<&TagRow>`. The [iterator parity test][live-iterator-test]
  reuses the frozen legacy-orphan fixture with two protected tags. Its full
  measurement equals the original slice result, and orphan tag 2 contributes
  nonzero total tokens but no unprotected tokens. The fixture is unchanged.
- The [capacity-refusal test][live-refusal-test] replaces an admitted entry
  with one whose spare source capacity exceeds a private cache budget. The
  old entry, retained charge, and LRU entry disappear; the loaded row remains
  usable. Readmission charges once. The independent exact formula remains in
  the [charge test][live-charge-test]: the 64-byte allocator allowance is
  separate from the two-counter Arc headers. No equal-capacity assumption
  across separate allocations is required.
- The [bootstrap mint test][live-bootstrap-test] already checks initial
  minting and replay. The [protection test][live-protection-test] also mints
  an untagged tail as tag 29 while stored tag 5 remains protected. Including
  the mint tail in bootstrap protection would displace tag 5 and apply its
  pending drop, contradicting the existing assertion.
- Final focused verification uses `cargo test -p daemon --lib --all-features
  --locked --` with filters `tag_baseline`, `tag_mint`, `tail_hygiene`,
  `claude_code_first_requested_surface_tags_bootstrap_pass_one`,
  `transform_projection_tag_numbers_include_same_pass_mints`,
  `mint_scope_matches_overlay_scope_and_captures_exact_source`, `pending`,
  and `differential_goldens`: 58 passed, zero failed, one ignored. The
  `caveman_differential`, `selection_differential`, and
  `historian_truncate_differential` integration targets pass with
  `--all-features --locked`: 24 passed. All-features daemon Clippy with
  `--all-targets --locked -- -D warnings` and scoped Rustfmt checks pass.
  These results do not replace the controller's final affected gate.

### Parent-merge verification

Execution provenance: 2026-09-12, working tree merging `0cf2fb3a` into
`2b83194f`. The earlier command counts above remain historical results.

- The merge retains the parent's test deduplication, including removal of
  the provenance-only mutation test. The shared-row iterator, mint-tail
  sharing, rollback, capacity, and bootstrap-protection checks remain.
  The auto-merged transform compiles without further source changes.
- Live anchors here and the conflicting catalog and check-inventory anchors
  are refreshed against the merged source. Discovery links remain pinned to
  the original baseline commit.
- `cargo test -p daemon --lib --all-features --locked -- tag_baseline tag_mint
  tail_hygiene differential_goldens
  claude_code_first_requested_surface_tags_bootstrap_pass_one
  transform_projection_tag_numbers_include_same_pass_mints
  mint_scope_matches_overlay_scope_and_captures_exact_source pending`
  passes 56 tests, with zero failures and one ignored manual timing test.
- `cargo test -p daemon --all-features --locked --test caveman_differential
  --test selection_differential --test historian_truncate_differential`
  passes 24 tests with zero failures.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
  passes. `rustfmt --edition 2024 --check` on `tail_hygiene.rs` and
  `transform.rs`, and `git diff --check`, pass.
- `differential_goldens.rs` is byte-identical to incoming parent `0cf2fb3a`,
  Git blob `7a32a3236bd3102dc764cd36616edfd7920c736c`. No schema, wire,
  guard, benchmark, or storage changes are added by this resolution.

[tc-tagnum]: ../../../daemon/transform/catalog.md#speculative-tag-numbering-has-two-authorities
[r1]: ../../catalog.md#prepared-field-output-and-audit-policy-agree
[load-call]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/transform.rs#L3002
[commit-inputs]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/transform.rs#L4924-L4934
[tag-entry]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/transform.rs#L6782-L6807
[tag-snapshot]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/transform.rs#L6827-L6832
[load-tags]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/transform.rs#L6899-L6957
[mint-input]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/transform.rs#L7156-L7161
[append-mint]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/transform.rs#L7261-L7282
[taggable]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/transform.rs#L7286-L7310
[active-match]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/transform.rs#L7350
[make-mut]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/transform.rs#L7883-L7884
[t-tagcold]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/transform.rs#L22418
[t-poison]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/transform.rs#L22487
[t-interleave]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/daemon/src/transform.rs#L22520
[store-existed]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/memory-store/src/lib.rs#L7140-L7145
[mint-prepared]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/memory-store/src/lib.rs#L7156-L7164
[store-number]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/memory-store/src/lib.rs#L7205-L7209
[load-order]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/crates/memory-store/src/lib.rs#L7245-L7273
[live-entry]: ../../../../../crates/daemon/src/transform.rs#L6836-L6861
[live-baseline]: ../../../../../crates/daemon/src/transform.rs#L3042-L3043
[live-protection]: ../../../../../crates/daemon/src/transform.rs#L3704-L3717
[live-tail]: ../../../../../crates/daemon/src/transform.rs#L7951-L7961
[live-combined]: ../../../../../crates/daemon/src/transform.rs#L3438-L3446
[live-hygiene]: ../../../../../crates/daemon/src/transform.rs#L8503-L8567
[live-measure]: ../../../../../crates/daemon/src/tail_hygiene.rs#L565-L580
[live-iterator-test]: ../../../../../crates/daemon/src/tail_hygiene.rs#L1283
[live-bootstrap-test]: ../../../../../crates/daemon/src/transform.rs#L21707
[live-protection-test]: ../../../../../crates/daemon/src/transform.rs#L23557
[live-refusal-test]: ../../../../../crates/daemon/src/transform.rs#L11856
[live-commit]: ../../../../../crates/daemon/src/transform.rs#L4959-L4968
[live-load]: ../../../../../crates/daemon/src/transform.rs#L6959-L7021
[live-charge]: ../../../../../crates/daemon/src/transform.rs#L6926-L6942
[live-charge-test]: ../../../../../crates/daemon/src/transform.rs#L11893
[live-interleave]: ../../../../../crates/daemon/src/transform.rs#L22626
[live-sharing]: ../../../../../crates/daemon/src/transform.rs#L22673
[live-rollback]: ../../../../../crates/daemon/src/transform.rs#L22710
[live-prepared]: ../../../../../crates/memory-store/src/lib.rs#L8234-L8249
[live-bytes]: ../../../../../crates/memory-store/src/lib.rs#L2079-L2086
[live-policy]: ../../../../../crates/memory-store/src/lib.rs#L2204-L2233
